//! Command blocks: with shell integration, the rows from one prompt to the
//! next are one command — its prompt line, what was typed, and its output —
//! with when it ran and how it ended. Worked out from the visible rows' OSC 133
//! marks: `A` starts a prompt; `C` the output, on the row after the prompt
//! line (which, for a command with no output, is the next prompt's row); and
//! `D` ends the command, just before the next prompt (so usually on its row).
use crate::terminal::TerminalRow;
use std::{sync::Arc, time::Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct CommandBlock {
    /// The prompt's row; `None` for rows above the first visible prompt.
    pub prompt: Option<usize>,
    /// Its first and last rows.
    pub rows: std::ops::RangeInclusive<usize>,
    /// Where its output starts, and when.
    pub output: Option<(usize, Instant)>,
    /// How it ended (its exit status, if the shell said), and when.
    pub finished: Option<(Option<i32>, Instant)>,
}

impl CommandBlock {
    /// A command ran (something was entered, not just Enter at a prompt).
    pub fn ran(&self) -> bool {
        self.output.is_some()
    }

    pub fn running(&self) -> bool {
        self.ran() && self.finished.is_none()
    }

    pub fn failed(&self) -> bool {
        matches!(self.finished, Some((Some(status), _)) if status != 0)
    }

    /// How long it ran (so far).
    pub fn duration(&self, now: Instant) -> Option<std::time::Duration> {
        let (_, started) = self.output?;
        let ended = self.finished.map_or(now, |(_, at)| at);
        Some(ended.saturating_duration_since(started))
    }
}

/// The blocks among `lines` (the visible rows, top to bottom). Without shell
/// integration there are no prompts, so no blocks.
pub fn command_blocks(lines: &[Arc<TerminalRow>]) -> Vec<CommandBlock> {
    // Every `A` is a prompt: after a command with no output, the next prompt
    // is on the very next row.
    let prompts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.prompt)
        .map(|(row, _)| row)
        .collect();
    if prompts.is_empty() {
        return Vec::new();
    }
    let last = lines.len() - 1;
    let mut starts: Vec<(Option<usize>, usize)> = Vec::new();
    if prompts[0] > 0 {
        starts.push((None, 0));
    }
    starts.extend(prompts.iter().map(|&row| (Some(row), row)));
    starts
        .iter()
        .enumerate()
        .map(|(index, &(prompt, first))| {
            let next = starts.get(index + 1).map(|&(_, row)| row);
            let end = next.map_or(last, |row| row - 1);
            // After the prompt line, up to the next prompt's row.
            let from = if prompt.is_some() { first + 1 } else { first };
            let output = (from..=next.unwrap_or(last))
                .find_map(|row| lines.get(row)?.command.started.map(|at| (row, at)));
            // Its end: after it started, up to the next prompt's row.
            let finished = output.and_then(|(from, _)| {
                (from..=next.unwrap_or(last)).find_map(|row| lines[row].command.finished)
            });
            CommandBlock {
                prompt,
                rows: first..=end,
                output,
                finished,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::CommandMark;

    fn row(prompt: bool, command: CommandMark) -> Arc<TerminalRow> {
        Arc::new(TerminalRow {
            revision: 0,
            cells: Vec::new(),
            wrapped: false,
            prompt,
            command,
        })
    }

    #[test]
    fn prompts_split_rows_into_commands_as_zsh_marks_them() {
        let t0 = Instant::now();
        let at = |seconds: f32| t0 + std::time::Duration::from_secs_f32(seconds);
        let mark = |started: Option<f32>, finished: Option<(i32, f32)>| CommandMark {
            started: started.map(at),
            finished: finished.map(|(status, seconds)| (Some(status), at(seconds))),
        };
        // What zsh's marks look like (from a real session):
        let lines = vec![
            row(false, mark(None, None)),          // the end of an earlier command
            row(true, mark(None, None)),           // $ echo first
            row(false, mark(Some(0.0), None)),     // first
            row(true, mark(None, Some((0, 0.1)))), // $ false
            row(true, mark(Some(1.0), Some((1, 1.0)))), // $ sleep 2 (false's C and D)
            row(false, mark(Some(2.0), None)),     // slept
            row(true, mark(None, Some((0, 4.0)))), // $
        ];
        let blocks = command_blocks(&lines);
        assert_eq!(blocks.len(), 5, "{blocks:?}");
        assert_eq!((blocks[0].prompt, blocks[0].rows.clone()), (None, 0..=0));
        assert!(!blocks[0].ran());
        assert_eq!(blocks[1].rows, 1..=2);
        assert!(matches!(blocks[1].finished, Some((Some(0), _))));
        // `false` printed nothing: its marks are on the next prompt's row.
        assert_eq!(blocks[2].rows, 3..=3);
        assert_eq!(blocks[2].output.map(|(row, _)| row), Some(4));
        assert!(blocks[2].failed());
        assert_eq!(blocks[3].rows, 4..=5);
        assert_eq!(blocks[3].output.map(|(row, _)| row), Some(5));
        assert_eq!(
            blocks[3].duration(t0),
            Some(std::time::Duration::from_secs(2))
        );
        assert!(!blocks[3].failed());
        // The current prompt: nothing ran yet.
        assert!(!blocks[4].ran() && !blocks[4].running());
        assert!(command_blocks(&[row(false, mark(Some(0.0), None))]).is_empty());
    }
}
