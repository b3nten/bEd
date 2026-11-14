//! Profiler-inspired views: a *timeline* (schedule bands, topological waves as time slots,
//! parallel systems stacked like threads, critical path highlighted) and a *flame chart* of the
//! frame's structure (frame → schedule → sets → systems, click to zoom).

use std::collections::{HashMap, HashSet};

use bedtime_ui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Rect, Sense, Shape, Stroke, StrokeKind,
    TextStyle, epaint::CubicBezierShape, pos2, vec2,
};

use super::{
    ViewState, comp_name, frame_view::ensure_frame, named_sets, scene_text::SceneText,
    search_matches, style,
};
use crate::{
    frame::{BandKind, FrameSequence},
    layout::visible::primary_chains,
    snapshot::{AppSnapshot, NodeRef, ScheduleSnapshot, SysIx},
};

const BAR_H: f32 = 22.0;
const ROW_GAP: f32 = 4.0;
const WAVE_GAP: f32 = 18.0;

enum Action {
    Select(String, NodeRef),
    Hover(Option<NodeRef>),
    Zoom(Option<usize>),
}

fn band_color(kind: BandKind, label: &str, alpha: u8) -> Color32 {
    let (s, v) = match kind {
        BandKind::Main => (0.45, 0.6),
        BandKind::Fixed => (0.55, 0.75),
        BandKind::Transition => (0.35, 0.8),
        BandKind::Startup => (0.2, 0.6),
        BandKind::Other => (0.1, 0.5),
    };
    style::hue_color(label, s, v, alpha)
}

/// Systems that lie on a longest dependency chain of their schedule.
fn critical_systems(schedule: &ScheduleSnapshot) -> HashSet<SysIx> {
    let n = schedule.systems.len();
    let ranks = crate::layout::ranks_of(schedule);
    let mut longest_from = vec![0u32; n];
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(ranks[i]));
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &schedule.flat_edges {
        out[e.from as usize].push(e.to as usize);
    }
    for i in order {
        longest_from[i] = out[i]
            .iter()
            .map(|&j| longest_from[j] + 1)
            .max()
            .unwrap_or(0);
    }
    let total = (0..n)
        .map(|i| ranks[i] + longest_from[i])
        .max()
        .unwrap_or(0);
    if total == 0 {
        return HashSet::new();
    }
    (0..n)
        .filter(|&i| ranks[i] + longest_from[i] == total)
        .map(|i| i as SysIx)
        .collect()
}

struct Bar {
    column: usize,
    rect: Rect,
    /// Measured duration of this run (measured mode only).
    duration: Option<std::time::Duration>,
}

pub(super) const TRACK_LABEL_W: f32 = 210.0;
const TRACK_GAP: f32 = 10.0;

struct Track {
    band: usize,
    rect: Rect,
    waves: u32,
    /// Measured time spent in this schedule during the frame (measured mode only).
    measured: Option<std::time::Duration>,
}

/// Bars, tracks, bounds and (for measured layouts) the time axis.
type Placed = (Vec<Bar>, Vec<Track>, Rect, Option<TimeAxis>);

/// Time axis of a measured timeline.
struct TimeAxis {
    x0: f32,
    px_per_us: f32,
    duration: std::time::Duration,
    /// Longest single run in the frame (over every system, hidden ones included); heat colours
    /// are normalised against it.
    max_run: std::time::Duration,
}

/// Places every recorded run of the last frame at its real time; rows are threads.
#[cfg(feature = "profile")]
fn place_bars_measured(
    snapshot: &AppSnapshot,
    seq: &FrameSequence,
    frame: &crate::profile::FrameTimings,
    time_zoom: f32,
) -> Option<(Vec<Bar>, Vec<Track>, Rect, TimeAxis)> {
    let total_us = frame.duration.as_micros() as f32;
    if total_us <= 0.0 || frame.systems.is_empty() {
        return None;
    }
    let x0 = TRACK_LABEL_W;
    let px_per_us = 2200.0 * time_zoom.max(0.01) / total_us;
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for fs in &seq.systems {
        let name = snapshot.schedules[fs.schedule].systems[fs.system as usize]
            .name
            .as_str();
        by_name.entry(name).or_default().push(fs.column);
    }
    let band_of_column = |col: usize| {
        seq.bands
            .iter()
            .position(|b| b.first <= col && col < b.last)
    };
    let schedule_of_time = |t: std::time::Duration| -> Option<&str> {
        frame
            .schedules
            .iter()
            .filter(|s| s.start <= t && t <= s.end)
            .min_by_key(|s| s.end - s.start)
            .map(|s| s.label.as_str())
    };
    // (band, thread) -> row
    let mut rows: HashMap<(usize, u32), usize> = HashMap::new();
    let mut rows_per_band: HashMap<usize, usize> = HashMap::new();
    let mut placed: Vec<(usize, usize, f32, f32, std::time::Duration)> = Vec::new();
    let mut placed_cols: Vec<usize> = Vec::new();
    for run in &frame.systems {
        let Some(cols) = by_name.get(run.name.as_str()) else {
            continue;
        };
        let label = schedule_of_time(run.start);
        let col = cols
            .iter()
            .copied()
            .find(|c| {
                band_of_column(*c).is_some_and(|b| Some(seq.bands[b].label.as_str()) == label)
            })
            .or_else(|| cols.first().copied());
        let Some(col) = col else { continue };
        let Some(band) = band_of_column(col) else {
            continue;
        };
        let row = *rows.entry((band, run.thread)).or_insert_with(|| {
            let r = rows_per_band.entry(band).or_default();
            *r += 1;
            *r - 1
        });
        let x = x0 + run.start.as_micros() as f32 * px_per_us;
        let w = ((run.end.saturating_sub(run.start)).as_micros() as f32 * px_per_us).max(4.0);
        placed.push((band, row, x, w, run.end.saturating_sub(run.start)));
        placed_cols.push(col);
    }
    let mut tracks = Vec::new();
    let mut band_y: HashMap<usize, f32> = HashMap::new();
    let mut y = 0.0f32;
    let x_end = x0 + total_us * px_per_us;
    for (bi, band) in seq.bands.iter().enumerate() {
        let Some(&rows) = rows_per_band.get(&bi) else {
            continue;
        };
        let track_h = 8.0 + rows as f32 * (BAR_H + ROW_GAP) + 4.0;
        band_y.insert(bi, y);
        let measured = frame
            .schedules
            .iter()
            .filter(|s| s.label == band.label)
            .map(|s| s.end.saturating_sub(s.start))
            .sum::<std::time::Duration>();
        tracks.push(Track {
            band: bi,
            rect: Rect::from_min_max(pos2(0.0, y), pos2(x_end + 10.0, y + track_h)),
            waves: seq.systems[band.first..band.last]
                .iter()
                .map(|s| s.wave)
                .max()
                .map_or(0, |w| w + 1),
            measured: Some(measured),
        });
        y += track_h + TRACK_GAP;
    }
    let bars = placed
        .into_iter()
        .zip(placed_cols)
        .map(|((band, row, x, w, duration), column)| Bar {
            column,
            duration: Some(duration),
            rect: Rect::from_min_size(
                pos2(x, band_y[&band] + 8.0 + row as f32 * (BAR_H + ROW_GAP)),
                vec2(w, BAR_H),
            ),
        })
        .collect();
    let bounds = Rect::from_min_max(pos2(-10.0, -30.0), pos2(x_end + 30.0, y + 20.0));
    let max_run = frame
        .systems
        .iter()
        .map(|s| s.end.saturating_sub(s.start))
        .max()
        .unwrap_or_default();
    Some((
        bars,
        tracks,
        bounds,
        TimeAxis {
            x0,
            px_per_us,
            duration: frame.duration,
            max_run,
        },
    ))
}

/// Places every system: one track per schedule (top to bottom in frame order), waves as time
/// slots left to right inside the track, parallel systems stacked within the slot.
fn place_bars(
    snapshot: &AppSnapshot,
    seq: &FrameSequence,
    measure: &mut dyn FnMut(&str) -> f32,
) -> (Vec<Bar>, Vec<Track>, Rect) {
    let mut bars = Vec::new();
    let mut tracks = Vec::new();
    let x0 = TRACK_LABEL_W;
    let mut y = 0.0f32;
    let mut max_x: f32 = x0;
    for (bi, band) in seq.bands.iter().enumerate() {
        if band.first == band.last {
            continue;
        }
        let systems = &seq.systems[band.first..band.last];
        let waves = systems.iter().map(|s| s.wave).max().unwrap_or(0) + 1;
        let mut wave_widths = vec![0.0f32; waves as usize];
        let mut wave_rows = vec![0usize; waves as usize];
        for fs in systems {
            let system = &snapshot.schedules[fs.schedule].systems[fs.system as usize];
            let w = measure(&super::frame_view::truncate(&system.short_name, 30)) + 16.0;
            let wave = fs.wave as usize;
            wave_widths[wave] = wave_widths[wave].max(w).max(50.0);
            wave_rows[wave] += 1;
        }
        let mut wave_x = vec![0.0f32; waves as usize];
        let mut cursor = x0 + 8.0;
        for w in 0..waves as usize {
            wave_x[w] = cursor;
            cursor += wave_widths[w] + WAVE_GAP;
        }
        let rows = wave_rows.iter().copied().max().unwrap_or(1).max(1);
        let track_h = 8.0 + rows as f32 * (BAR_H + ROW_GAP) + 4.0;
        let mut next_row = vec![0usize; waves as usize];
        for fs in systems {
            let wave = fs.wave as usize;
            let row = next_row[wave];
            next_row[wave] += 1;
            let rect = Rect::from_min_size(
                pos2(wave_x[wave], y + 8.0 + row as f32 * (BAR_H + ROW_GAP)),
                vec2(wave_widths[wave], BAR_H),
            );
            bars.push(Bar {
                column: fs.column,
                rect,
                duration: None,
            });
        }
        let x1 = cursor - WAVE_GAP + 8.0;
        max_x = max_x.max(x1);
        tracks.push(Track {
            band: bi,
            rect: Rect::from_min_max(pos2(0.0, y), pos2(x1, y + track_h)),
            waves,
            measured: None,
        });
        y += track_h + TRACK_GAP;
    }
    for track in &mut tracks {
        track.rect.max.x = max_x;
    }
    let bounds = Rect::from_min_max(pos2(-10.0, -10.0), pos2(max_x + 20.0, y + 20.0));
    (bars, tracks, bounds)
}

pub fn show_timeline(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ensure_frame(snapshot, view);
    let font = TextStyle::Body.resolve(ui.style());
    let Some(cache) = view.frame_cache.take() else {
        return;
    };
    let seq = &cache.seq;
    let painter = ui.painter().clone();
    let width_cache = &mut view.width_cache;
    let mut measure = |text: &str| -> f32 {
        *width_cache.entry(text.to_string()).or_insert_with(|| {
            painter
                .layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    #[allow(unused_mut)]
    let mut placed: Option<Placed> = None;
    #[cfg(feature = "profile")]
    if view.timeline_measured
        && let Some(timings) = view.timings.as_ref()
        && let Some(frame) = timings.lock().ok().and_then(|s| {
            if view.use_slowest_frame {
                s.slowest_frame().cloned()
            } else {
                s.last().cloned()
            }
        })
        && let Some((bars, tracks, bounds, axis)) =
            place_bars_measured(snapshot, seq, &frame, view.time_zoom)
    {
        placed = Some((bars, tracks, bounds, Some(axis)));
    }
    let (bars, tracks, bounds, axis) = placed.unwrap_or_else(|| {
        let (bars, tracks, bounds) = place_bars(snapshot, seq, &mut measure);
        (bars, tracks, bounds, None)
    });
    // Switching between the structural and the measured layout changes the canvas size.
    if view.timeline_was_measured != axis.is_some() {
        view.timeline_was_measured = axis.is_some();
        view.frame_fit_requested = true;
    }
    if view.frame_fit_requested {
        view.frame_scene_rect = bounds;
        view.frame_fit_requested = false;
    }
    let mut scene_rect = view.frame_scene_rect;
    let mut actions = Vec::new();
    egui::Scene::new()
        .zoom_range(0.05..=4.0)
        .show(ui, &mut scene_rect, |ui| {
            draw_timeline(
                ui,
                snapshot,
                view,
                seq,
                &bars,
                &tracks,
                axis.as_ref(),
                &font,
                &mut actions,
            );
        });
    view.frame_scene_rect = scene_rect;
    view.frame_cache = Some(cache);
    for action in actions {
        match action {
            Action::Select(label, node) => view.select_in_schedule(label, node),
            Action::Hover(node) => view.hovered = node,
            Action::Zoom(_) => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_timeline(
    ui: &mut egui::Ui,
    snapshot: &AppSnapshot,
    view: &ViewState,
    seq: &FrameSequence,
    bars: &[Bar],
    tracks: &[Track],
    axis: Option<&TimeAxis>,
    font: &FontId,
    actions: &mut Vec<Action>,
) {
    let clip = ui.clip_rect();
    let painter = ui.painter().clone();
    let text = SceneText::new(ui);
    let bar_of_column: HashMap<usize, usize> = bars
        .iter()
        .enumerate()
        .map(|(i, b)| (b.column, i))
        .collect();

    // Time axis (measured mode).
    if let Some(axis) = axis {
        let total_ms = axis.duration.as_secs_f32() * 1000.0;
        let step_ms = [0.1f32, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .find(|s| total_ms / s <= 24.0)
            .unwrap_or(20.0);
        let mut t = 0.0f32;
        while t <= total_ms + 1e-3 {
            let x = axis.x0 + t * 1000.0 * axis.px_per_us;
            painter.line_segment(
                [pos2(x, -18.0), pos2(x, clip.max.y)],
                Stroke::new(1.0, style::node_text().gamma_multiply(0.07)),
            );
            text.text(
                pos2(x + 2.0, -18.0),
                Align2::LEFT_TOP,
                format!("{t:.2} ms"),
                &FontId::proportional(font.size * 0.8),
                style::filter(),
            );
            t += step_ms;
        }
        text.text(
            pos2(4.0, -18.0),
            Align2::LEFT_TOP,
            format!("frame {total_ms:.2} ms"),
            &FontId::proportional(font.size * 0.85),
            style::node_text(),
        );
    }
    let search = view.search.to_lowercase();

    // Interaction.
    let mut hovered: Option<usize> = None;
    for (i, bar) in bars.iter().enumerate() {
        if !bar.rect.intersects(clip) {
            continue;
        }
        let resp = ui.interact(bar.rect, ui.id().with(("tl_bar", i)), Sense::click());
        if resp.hovered() {
            hovered = Some(i);
        }
        if resp.clicked() {
            let fs = seq.systems[bar.column];
            actions.push(Action::Select(
                snapshot.schedules[fs.schedule].label.clone(),
                NodeRef::System(fs.system),
            ));
        }
    }
    let selected: Option<usize> = view.selected.and_then(|node| {
        let NodeRef::System(sys) = node else {
            return None;
        };
        let label = view.schedule.as_deref()?;
        bars.iter().position(|b| {
            let fs = seq.systems[b.column];
            fs.system == sys && snapshot.schedules[fs.schedule].label == label
        })
    });
    let focus = hovered.or(selected);
    actions.push(Action::Hover(
        hovered.map(|i| NodeRef::System(seq.systems[bars[i].column].system)),
    ));

    // Tracks (one per schedule).
    for track in tracks {
        let band = &seq.bands[track.band];
        if !track.rect.intersects(clip) {
            continue;
        }
        painter.rect_filled(
            track.rect,
            CornerRadius::same(4),
            band_color(band.kind, &band.label, 22),
        );
        let label_rect =
            Rect::from_min_max(track.rect.min, pos2(TRACK_LABEL_W - 6.0, track.rect.max.y));
        painter.rect_filled(
            label_rect,
            CornerRadius::same(4),
            band_color(band.kind, &band.label, 50),
        );
        let indent = 8.0 + band.depth as f32 * 14.0;
        let title = super::frame_view::truncate(&band.label, 24);
        text.text(
            label_rect.min + vec2(indent, 6.0),
            Align2::LEFT_TOP,
            title,
            font,
            Color32::WHITE,
        );
        let kind = match band.kind {
            BandKind::Fixed => "fixed · 0..n per frame",
            BandKind::Transition => "on state change",
            BandKind::Startup => "startup",
            _ => "",
        };
        let count = band.last - band.first;
        let info = match track.measured {
            Some(d) => format!(
                "{:.3} ms · {count} systems {kind}",
                d.as_secs_f64() * 1000.0
            ),
            None => format!("{count} systems · {} waves {kind}", track.waves),
        };
        painter.text(
            label_rect.min + vec2(indent, 24.0),
            Align2::LEFT_TOP,
            info,
            FontId::proportional(font.size * 0.8),
            style::node_text_alt(),
        );
    }

    // Critical path per schedule.
    let mut critical: HashMap<usize, HashSet<SysIx>> = HashMap::new();
    for (si, schedule) in snapshot.schedules.iter().enumerate() {
        critical.insert(si, critical_systems(schedule));
    }

    // Edges: ordering (within schedule) for the focused bar; data hand-offs for the focused bar.
    if let Some(f) = focus {
        let fcol = bars[f].column;
        let fs = seq.systems[fcol];
        let schedule = &snapshot.schedules[fs.schedule];
        let col_of: HashMap<SysIx, usize> = seq.systems[..]
            .iter()
            .filter(|s| s.schedule == fs.schedule)
            .map(|s| (s.system, s.column))
            .collect();
        for e in &schedule.flat_edges {
            if e.from != fs.system && e.to != fs.system {
                continue;
            }
            let (Some(&a), Some(&b)) = (col_of.get(&e.from), col_of.get(&e.to)) else {
                continue;
            };
            let (Some(&ba), Some(&bb)) = (bar_of_column.get(&a), bar_of_column.get(&b)) else {
                continue;
            };
            let color = if e.to == fs.system {
                style::edge_in()
            } else {
                style::edge_out()
            };
            connect(&painter, bars[ba].rect, bars[bb].rect, color, 2.0, false);
        }
        for h in &seq.handoffs {
            let involved = h.from == fcol || h.to.contains(&fcol);
            if !involved {
                continue;
            }
            let color =
                style::hue_color(&snapshot.components[h.comp as usize].name, 0.75, 1.0, 230);
            for t in &h.to {
                if h.from != fcol && *t != fcol {
                    continue;
                }
                let (Some(&ba), Some(&bb)) = (bar_of_column.get(&h.from), bar_of_column.get(t))
                else {
                    continue;
                };
                connect(&painter, bars[ba].rect, bars[bb].rect, color, 1.6, true);
                let mid = pos2(
                    (bars[ba].rect.max.x + bars[bb].rect.min.x) / 2.0,
                    (bars[ba].rect.center().y + bars[bb].rect.center().y) / 2.0 - 8.0,
                );
                text.text(
                    mid,
                    Align2::CENTER_BOTTOM,
                    comp_name(snapshot, h.comp),
                    &FontId::proportional(font.size * 0.8),
                    color,
                );
            }
        }
    }

    // Bars.
    let max_duration = axis.map(|a| a.max_run);
    for (i, bar) in bars.iter().enumerate() {
        if !bar.rect.intersects(clip) {
            continue;
        }
        let fs = seq.systems[bar.column];
        let schedule = &snapshot.schedules[fs.schedule];
        let system = &schedule.systems[fs.system as usize];
        let sets = named_sets(schedule, system);
        let base = match sets.last() {
            Some(set) => style::hue_color(set, 0.45, 0.55, 255),
            None => style::node_fill(),
        };
        let matches = search.is_empty() || search_matches(snapshot, system, &search);
        let mut fill = base;
        if view.heat_colours
            && let Some(d) = bar.duration
            && let Some(max) = max_duration
        {
            let t = (d.as_micros().max(1) as f32).ln() / (max.as_micros().max(2) as f32).ln();
            fill = style::heat_color(t);
        }
        if focus == Some(i) {
            fill = style::node_fill_hover();
        }
        if !matches {
            fill = fill.gamma_multiply(0.45);
        }
        // The critical path is a structural notion; it only decorates the wave layout.
        let is_critical = axis.is_none()
            && view.show_critical_path
            && critical
                .get(&fs.schedule)
                .is_some_and(|c| c.contains(&fs.system));
        let thin = bar.rect.width() < 8.0;
        let stroke = if selected == Some(i) {
            Stroke::new(2.0, style::selected())
        } else if thin {
            Stroke::NONE
        } else if is_critical {
            Stroke::new(1.5, style::ambiguity())
        } else if system.flags.exclusive {
            Stroke::new(1.2, style::exclusive())
        } else {
            Stroke::new(1.0, style::node_stroke())
        };
        painter.rect_filled(bar.rect, CornerRadius::same(4), fill);
        painter.rect_stroke(bar.rect, CornerRadius::same(4), stroke, StrokeKind::Inside);
        let text_color = if matches {
            style::node_text()
        } else {
            style::node_text_dim()
        };
        let galley = text.layout(&system.short_name, font, text_color);
        let size = text.size(&galley);
        let text_pos = pos2(bar.rect.min.x + 6.0, bar.rect.center().y - size.y / 2.0);
        if size.x + 10.0 <= bar.rect.width() {
            text.galley(text_pos, galley, text_color);
        } else if bar.rect.width() > 24.0 {
            text.clipped(bar.rect.shrink2(vec2(3.0, 0.0)))
                .galley(text_pos, galley, text_color);
        }
        if system.flags.deferred {
            text.text(
                bar.rect.max - vec2(4.0, BAR_H / 2.0),
                Align2::RIGHT_CENTER,
                "⬇",
                &FontId::proportional(font.size * 0.8),
                style::filter(),
            );
        }
    }

    if let Some(i) = hovered {
        let resp = ui.interact(bars[i].rect, ui.id().with(("tl_tip", i)), Sense::hover());
        resp.on_hover_ui_at_pointer(|ui| {
            let fs = seq.systems[bars[i].column];
            let schedule = &snapshot.schedules[fs.schedule];
            let system = &schedule.systems[fs.system as usize];
            ui.set_max_width(420.0);
            ui.monospace(&system.name);
            ui.colored_label(
                style::filter(),
                format!("{} · wave {}", schedule.label, fs.wave),
            );
            let sets = named_sets(schedule, system);
            if !sets.is_empty() {
                ui.colored_label(style::filter(), format!("sets: {}", sets.join(" › ")));
            }
            if critical
                .get(&fs.schedule)
                .is_some_and(|c| c.contains(&fs.system))
            {
                ui.colored_label(style::ambiguity(), "on the critical path of this schedule");
            }
            #[cfg(feature = "profile")]
            if let Some(timings) = view.timings.as_ref()
                && let Ok(store) = timings.lock()
                && let Some(frame) = store.last()
            {
                let runs: Vec<_> = frame.systems.iter().filter(|s| s.name == system.name).collect();
                if !runs.is_empty() {
                    let last: std::time::Duration =
                        runs.iter().map(|s| s.end.saturating_sub(s.start)).sum();
                    let avg = store.average(&system.name).unwrap_or_default();
                    ui.colored_label(
                        style::warn(),
                        format!(
                            "last frame {:.3} ms in {} run{} · avg {:.3} ms over {} frames · thread {}",
                            last.as_secs_f64() * 1000.0,
                            runs.len(),
                            if runs.len() == 1 { "" } else { "s" },
                            avg.as_secs_f64() * 1000.0,
                            store.frames.len(),
                            runs[0].thread
                        ),
                    );
                }
            }
        });
    }
}

fn connect(painter: &Painter, from: Rect, to: Rect, color: Color32, width: f32, dashed: bool) {
    let p0 = pos2(from.max.x, from.center().y);
    let p3 = pos2(to.min.x, to.center().y);
    let dx = ((p3.x - p0.x).abs() * 0.5).max(20.0);
    let pts = [p0, p0 + vec2(dx, 0.0), p3 - vec2(dx, 0.0), p3];
    let shape = CubicBezierShape::from_points_stroke(
        pts,
        false,
        Color32::TRANSPARENT,
        Stroke::new(width, color),
    );
    if dashed {
        let flat = shape.flatten(Some(0.5));
        painter.extend(Shape::dashed_line(
            &flat,
            Stroke::new(width, color),
            6.0,
            4.0,
        ));
    } else {
        painter.add(shape);
    }
    let dir = (pts[3] - pts[2]).normalized();
    let normal = vec2(-dir.y, dir.x);
    let base = p3 - dir * 7.0;
    painter.add(Shape::convex_polygon(
        vec![p3, base + normal * 3.5, base - normal * 3.5],
        color,
        Stroke::NONE,
    ));
}

// ------------------------------------------------------------------------------------------------
// Slowest systems (measured)
// ------------------------------------------------------------------------------------------------

/// Side-panel list of the slowest systems over the kept frames; clicking one selects it.
#[cfg(feature = "profile")]
pub fn slowest_list(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ensure_frame(snapshot, view);
    let Some(timings) = view.timings.clone() else {
        return;
    };
    let (stats, frame_avg, frame_max, frames) = {
        let store = timings.lock().unwrap_or_else(|p| p.into_inner());
        (
            store.slowest_systems(12),
            store.average_frame().unwrap_or_default(),
            store
                .slowest_frame()
                .map(|f| f.duration)
                .unwrap_or_default(),
            store.frames.len(),
        )
    };
    if frames == 0 {
        ui.colored_label(style::filter(), "no frames recorded yet");
        return;
    }
    ui.label(
        egui::RichText::new(format!(
            "frame avg {:.2} ms · max {:.2} ms · {frames} frames",
            frame_avg.as_secs_f64() * 1000.0,
            frame_max.as_secs_f64() * 1000.0
        ))
        .small()
        .color(style::filter()),
    );
    let frame_us = frame_avg.as_micros().max(1) as f64;
    let mut select: Option<String> = None;
    for stat in stats {
        let short = bevy::utils::prelude::ShortName(&stat.name).to_string();
        let pct = stat.average.as_micros() as f64 / frame_us * 100.0;
        let selected = matches!(
            (view.selected, view.schedule.as_deref()),
            (Some(NodeRef::System(sys)), Some(label))
                if snapshot.schedule(label).is_some_and(|s| s.systems.get(sys as usize).is_some_and(|x| x.name == stat.name))
        );
        let text = format!(
            "{:>7.3} ms {pct:>4.1}%  {short}",
            stat.average.as_secs_f64() * 1000.0
        );
        if ui
            .selectable_label(selected, egui::RichText::new(text).monospace().small())
            .on_hover_text(format!(
                "{}\nlast {:.3} ms · max {:.3} ms · ran in {} of {frames} frames",
                stat.name,
                stat.last.as_secs_f64() * 1000.0,
                stat.max.as_secs_f64() * 1000.0,
                stat.frames
            ))
            .clicked()
        {
            select = Some(stat.name.clone());
        }
    }
    if let Some(name) = select {
        let target = view.frame_cache.as_ref().and_then(|c| {
            c.seq.systems.iter().find_map(|fs| {
                let schedule = &snapshot.schedules[fs.schedule];
                (schedule.systems[fs.system as usize].name == name)
                    .then(|| (schedule.label.clone(), fs.system))
            })
        });
        match target {
            Some((label, sys)) => view.select_in_schedule(label, NodeRef::System(sys)),
            None => {
                // Not in the frame sequence (e.g. an internal system while internals are hidden).
                view.show_internal = true;
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Flame chart
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FlameNode {
    label: String,
    depth: usize,
    /// Leaf systems below this node.
    weight: usize,
    children: Vec<usize>,
    schedule: Option<usize>,
    system: Option<SysIx>,
}

fn build_flame(snapshot: &AppSnapshot, seq: &FrameSequence) -> Vec<FlameNode> {
    let mut nodes = vec![FlameNode {
        label: "frame".into(),
        depth: 0,
        weight: 0,
        children: Vec::new(),
        schedule: None,
        system: None,
    }];
    for band in &seq.bands {
        if band.first == band.last {
            continue;
        }
        let schedule = &snapshot.schedules[band.schedule];
        let chains = primary_chains(schedule);
        let band_node = nodes.len();
        nodes.push(FlameNode {
            label: band.label.clone(),
            depth: 1,
            weight: 0,
            children: Vec::new(),
            schedule: Some(band.schedule),
            system: None,
        });
        nodes[0].children.push(band_node);
        // Set nodes are created lazily per (band, set path).
        let mut set_nodes: HashMap<Vec<u32>, usize> = HashMap::new();
        for fs in &seq.systems[band.first..band.last] {
            let chain = &chains[fs.system as usize];
            let mut parent = band_node;
            let mut path = Vec::new();
            for set in chain {
                path.push(*set);
                let node = *set_nodes.entry(path.clone()).or_insert_with(|| {
                    let node = nodes.len();
                    nodes.push(FlameNode {
                        label: schedule.sets[*set as usize].name.clone(),
                        depth: nodes[parent].depth + 1,
                        weight: 0,
                        children: Vec::new(),
                        schedule: Some(band.schedule),
                        system: None,
                    });
                    nodes[parent].children.push(node);
                    node
                });
                parent = node;
            }
            let leaf = nodes.len();
            let system = &schedule.systems[fs.system as usize];
            nodes.push(FlameNode {
                label: system.short_name.clone(),
                depth: nodes[parent].depth + 1,
                weight: 1,
                children: Vec::new(),
                schedule: Some(band.schedule),
                system: Some(fs.system),
            });
            nodes[parent].children.push(leaf);
        }
    }
    // Weights bottom-up (children were pushed after parents, so iterate in reverse).
    for i in (0..nodes.len()).rev() {
        let sum: usize = nodes[i].children.iter().map(|&c| nodes[c].weight).sum();
        if !nodes[i].children.is_empty() {
            nodes[i].weight = sum;
        }
    }
    nodes
}

pub fn show_flame(ui: &mut egui::Ui, snapshot: &AppSnapshot, view: &mut ViewState) {
    ensure_frame(snapshot, view);
    let font = TextStyle::Body.resolve(ui.style());
    let Some(cache) = view.frame_cache.take() else {
        return;
    };
    let nodes = build_flame(snapshot, &cache.seq);
    view.frame_cache = Some(cache);

    let root = view.flame_root.filter(|r| *r < nodes.len()).unwrap_or(0);
    let area = ui.available_rect_before_wrap();
    let row_h = 26.0;
    let painter = ui.painter().clone();
    let mut actions = Vec::new();
    let mut hovered: Option<usize> = None;
    let search = view.search.to_lowercase();

    // Breadcrumb / reset.
    ui.horizontal(|ui| {
        if ui.small_button("whole frame").clicked() {
            actions.push(Action::Zoom(None));
        }
        ui.colored_label(
            style::filter(),
            format!("{} systems under {}", nodes[root].weight, nodes[root].label),
        );
        ui.checkbox(&mut view.flame_show_sets, "show sets");
    });
    let area = Rect::from_min_max(pos2(area.min.x, area.min.y + 24.0), area.max);
    let total = nodes[root].weight.max(1) as f32;

    // Depth-first layout from the zoom root.
    let mut stack: Vec<(usize, f32, f32, usize)> = vec![(root, area.min.x, area.width(), 0)];
    let mut rects: Vec<(usize, Rect)> = Vec::new();
    while let Some((node, x, w, depth)) = stack.pop() {
        let rect = Rect::from_min_size(
            pos2(x, area.min.y + depth as f32 * row_h),
            vec2(w, row_h - 2.0),
        );
        rects.push((node, rect));
        let mut cursor = x;
        for &child in &nodes[node].children {
            let cw = w * nodes[child].weight as f32 / nodes[node].weight.max(1) as f32;
            if nodes[child].system.is_none()
                && !view.flame_show_sets
                && nodes[child].schedule.is_some()
                && nodes[child].depth > 1
            {
                // Skip set levels: hoist grandchildren.
                let mut inner = cursor;
                let mut sub: Vec<usize> = vec![child];
                let mut leaves = Vec::new();
                while let Some(s) = sub.pop() {
                    if nodes[s].system.is_some() {
                        leaves.push(s);
                    } else {
                        sub.extend(nodes[s].children.iter().rev());
                    }
                }
                leaves.reverse();
                for leaf in leaves {
                    let lw = w * nodes[leaf].weight as f32 / nodes[node].weight.max(1) as f32;
                    stack.push((leaf, inner, lw, depth + 1));
                    inner += lw;
                }
            } else {
                stack.push((child, cursor, cw, depth + 1));
            }
            cursor += cw;
        }
    }
    let _ = total;

    for (node, rect) in &rects {
        if rect.width() < 1.0 {
            continue;
        }
        let n = &nodes[*node];
        let resp = ui.interact(*rect, ui.id().with(("flame", *node)), Sense::click());
        if resp.hovered() {
            hovered = Some(*node);
        }
        if resp.clicked() {
            match (n.system, n.schedule) {
                (Some(sys), Some(si)) => actions.push(Action::Select(
                    snapshot.schedules[si].label.clone(),
                    NodeRef::System(sys),
                )),
                _ => actions.push(Action::Zoom(Some(*node))),
            }
        }
        let label_seed = n
            .schedule
            .map(|si| snapshot.schedules[si].label.as_str())
            .unwrap_or("frame");
        let (s, v) = if n.system.is_some() {
            (0.35, 0.5)
        } else {
            (0.5, 0.75 - 0.08 * n.depth.min(4) as f32)
        };
        let mut fill = style::hue_color(label_seed, s, v, 255);
        let matches = match (n.system, n.schedule) {
            (Some(sys), Some(si)) => {
                search.is_empty()
                    || search_matches(
                        snapshot,
                        &snapshot.schedules[si].systems[sys as usize],
                        &search,
                    )
            }
            _ => true,
        };
        if !matches {
            fill = fill.gamma_multiply(0.4);
        }
        if hovered == Some(*node) {
            fill = fill.gamma_multiply(1.3);
        }
        let selected = matches!((n.system, n.schedule, view.selected, view.schedule.as_deref()), (Some(sys), Some(si), Some(NodeRef::System(sel)), Some(label)) if sys == sel && snapshot.schedules[si].label == label);
        painter.rect_filled(*rect, CornerRadius::same(3), fill);
        if selected {
            painter.rect_stroke(
                *rect,
                CornerRadius::same(3),
                Stroke::new(2.0, style::selected()),
                StrokeKind::Inside,
            );
        }
        if rect.width() > 30.0 {
            let text = if n.system.is_some() {
                n.label.clone()
            } else {
                format!("{} ({})", n.label, n.weight)
            };
            let galley = painter.layout_no_wrap(text, font.clone(), Color32::WHITE);
            if galley.size().x < rect.width() - 8.0 {
                painter.galley(
                    pos2(rect.min.x + 4.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    Color32::WHITE,
                );
            } else {
                painter.with_clip_rect(rect.shrink2(vec2(4.0, 0.0))).galley(
                    pos2(rect.min.x + 4.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    Color32::WHITE,
                );
            }
        }
    }
    if let Some(node) = hovered {
        let n = &nodes[node];
        let rect = rects
            .iter()
            .find(|(k, _)| *k == node)
            .map(|(_, r)| *r)
            .unwrap_or(area);
        let resp = ui.interact(rect, ui.id().with(("flame_tip", node)), Sense::hover());
        resp.on_hover_ui_at_pointer(|ui| {
            ui.set_max_width(420.0);
            match (n.system, n.schedule) {
                (Some(sys), Some(si)) => {
                    let system = &snapshot.schedules[si].systems[sys as usize];
                    ui.monospace(&system.name);
                    ui.colored_label(style::filter(), &snapshot.schedules[si].label);
                    ui.colored_label(style::filter(), "click: details");
                }
                _ => {
                    ui.label(&n.label);
                    ui.colored_label(
                        style::filter(),
                        format!("{} systems · click to zoom", n.weight),
                    );
                }
            }
        });
    }
    ui.allocate_rect(area, Sense::hover());
    for action in actions {
        match action {
            Action::Select(label, node) => view.select_in_schedule(label, node),
            Action::Zoom(root) => view.flame_root = root,
            Action::Hover(_) => {}
        }
    }
    view.hovered = hovered.and_then(|h| nodes[h].system.map(NodeRef::System));
}
