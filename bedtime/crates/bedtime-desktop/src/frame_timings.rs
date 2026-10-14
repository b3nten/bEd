//! Opt-in CPU-side frame measurements. No file I/O occurs on a frame path.
use bevy::prelude::*;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

const CAPACITY: usize = 8192;

#[derive(Clone, Default, Resource)]
pub(crate) struct FrameTimings(Option<Arc<Mutex<Recorder>>>);

impl FrameTimings {
    pub fn from_environment() -> Self {
        Self(
            std::env::var_os("BED_FRAME_TIMINGS")
                .filter(|path| !path.is_empty())
                .map(|path| {
                    Arc::new(Mutex::new(Recorder {
                        path: path.into(),
                        origin: Instant::now(),
                        previous_frame: None,
                        records: VecDeque::with_capacity(CAPACITY),
                    }))
                }),
        )
    }
}

/// These measure CPU-side stages, not GPU completion or display presentation.
#[derive(Clone, Copy)]
pub enum FrameStage {
    Documents,
    Ui,
    Render,
}

struct Record {
    frame: u64,
    start_ms: f64,
    interval_ms: Option<f64>,
    documents_ms: f64,
    ui_ms: f64,
    render_ms: f64,
}

struct Recorder {
    path: PathBuf,
    origin: Instant,
    previous_frame: Option<Instant>,
    records: VecDeque<Record>,
}

impl Recorder {
    fn record(&mut self, frame: u64, start: Instant) -> &mut Record {
        let existing = self
            .records
            .back()
            .filter(|row| row.frame >= frame)
            .and_then(|_| {
                self.records
                    .iter()
                    .rposition(|record| record.frame == frame)
            });
        if let Some(index) = existing {
            return &mut self.records[index];
        }
        if self.records.len() == CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(Record {
            frame,
            start_ms: start.duration_since(self.origin).as_secs_f64() * 1000.0,
            interval_ms: None,
            documents_ms: 0.0,
            ui_ms: 0.0,
            render_ms: 0.0,
        });
        self.records.back_mut().unwrap()
    }

    fn export(&self) -> io::Result<()> {
        let mut output = BufWriter::new(File::create(&self.path)?);
        writeln!(
            output,
            "frame,start_ms,interval_ms,documents_ms,ui_ms,render_cpu_ms"
        )?;
        for row in &self.records {
            let interval = row
                .interval_ms
                .map(|value| format!("{value:.3}"))
                .unwrap_or_default();
            writeln!(
                output,
                "{},{:.3},{},{:.3},{:.3},{:.3}",
                row.frame, row.start_ms, interval, row.documents_ms, row.ui_ms, row.render_ms
            )?;
        }
        output.flush()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Err(error) = self.export() {
            eprintln!(
                "Bed: cannot write frame timings to {}: {error}",
                self.path.display()
            );
        }
    }
}

/// Retain through the measured stage. Disabled unless BED_FRAME_TIMINGS is set.
pub fn measure_frame_stage(world: &World, stage: FrameStage) -> Option<FrameStageTiming> {
    let recorder = world.get_resource::<FrameTimings>()?.0.as_ref()?.clone();
    let frame = match stage {
        FrameStage::Render => {
            world
                .get_resource::<crate::render::RenderFrame>()?
                .info
                .frame
                .0
        }
        _ => world.get_resource::<crate::DesktopFrameInfo>()?.frame.0 + 1,
    };
    let start = Instant::now();
    if matches!(stage, FrameStage::Ui) {
        let mut recorder = recorder.lock().unwrap();
        let interval = recorder
            .previous_frame
            .replace(start)
            .map(|previous| start.duration_since(previous).as_secs_f64() * 1000.0);
        recorder.record(frame, start).interval_ms = interval;
    }
    Some(FrameStageTiming {
        recorder,
        frame,
        stage,
        start,
    })
}

pub struct FrameStageTiming {
    recorder: Arc<Mutex<Recorder>>,
    frame: u64,
    stage: FrameStage,
    start: Instant,
}

impl Drop for FrameStageTiming {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64() * 1000.0;
        let mut recorder = self.recorder.lock().unwrap();
        let row = recorder.record(self.frame, self.start);
        match self.stage {
            FrameStage::Documents => row.documents_ms += elapsed,
            FrameStage::Ui => row.ui_ms += elapsed,
            FrameStage::Render => row.render_ms += elapsed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_timings_correlate_stages_and_bound_history() {
        let path =
            std::env::temp_dir().join(format!("bed-frame-timings-{}.csv", std::process::id()));
        let recorder = Arc::new(Mutex::new(Recorder {
            path: path.clone(),
            origin: Instant::now(),
            previous_frame: None,
            records: VecDeque::new(),
        }));
        let mut world = World::new();
        world.insert_resource(FrameTimings(Some(Arc::clone(&recorder))));
        world.insert_resource(crate::DesktopFrameInfo::default());
        let mut render = crate::render::RenderFrame::default();
        render.info.frame = crate::DesktopFrameId(1);
        world.insert_resource(render);
        for stage in [FrameStage::Documents, FrameStage::Ui, FrameStage::Render] {
            drop(measure_frame_stage(&world, stage).unwrap());
        }
        {
            let mut recorder = recorder.lock().unwrap();
            assert_eq!(recorder.records.len(), 1);
            let row = recorder.records.front().unwrap();
            assert_eq!(row.frame, 1);
            assert!(row.documents_ms >= 0.0 && row.ui_ms >= 0.0 && row.render_ms >= 0.0);
            for frame in 2..=CAPACITY as u64 + 1 {
                recorder.record(frame, Instant::now());
            }
            assert_eq!(recorder.records.len(), CAPACITY);
            assert_eq!(recorder.records.front().unwrap().frame, 2);
        }
        drop(world);
        drop(recorder);
        let csv = std::fs::read_to_string(&path).unwrap();
        assert_eq!(csv.lines().count(), CAPACITY + 1);
        assert!(csv.starts_with("frame,start_ms,interval_ms,documents_ms,ui_ms,render_cpu_ms\n"));
        std::fs::remove_file(path).unwrap();
    }
}
