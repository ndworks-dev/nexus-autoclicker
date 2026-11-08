//! The automation loop, run on a background thread.

use crate::input;
use crate::matcher::{Gray, Template};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Params {
    pub images: Vec<PathBuf>,
    pub max_loops: u32,
    pub click_interval: Duration,
    pub loop_interval: Duration,
    pub threshold: f32,
}

pub enum Event {
    Log(String),
    Status(String),
    Iteration(u32),
    Finished(String),
}

#[derive(Debug)]
enum Stop {
    Button,
    Escape,
}

impl Stop {
    fn message(&self) -> &'static str {
        match self {
            Stop::Button => "Stopped by the user.",
            Stop::Escape => "Interrupted (Esc).",
        }
    }
}

const START_DELAY: Duration = Duration::from_secs(1);
const MOVE_DURATION: Duration = Duration::from_millis(500);
/// Matches whose contrast is below this fraction of the template's are
/// treated as covered by a modal overlay (Nexus dims the page behind dialogs).
const MIN_CONTRAST: f32 = 0.7;

pub fn run(p: Params, stop: Arc<AtomicBool>, emit: impl Fn(Event)) {
    let message = match run_inner(&p, &stop, &emit) {
        Ok(()) => "Finished all iterations.".to_string(),
        Err(s) => s.message().to_string(),
    };
    emit(Event::Log(message.clone()));
    emit(Event::Finished(message));
}

fn run_inner(p: &Params, stop: &AtomicBool, emit: &impl Fn(Event)) -> Result<(), Stop> {
    let check = || -> Result<(), Stop> {
        if stop.load(Ordering::Relaxed) {
            Err(Stop::Button)
        } else if input::esc_pressed() {
            Err(Stop::Escape)
        } else {
            Ok(())
        }
    };
    let sleep = |d: Duration| -> Result<(), Stop> {
        let end = Instant::now() + d;
        loop {
            check()?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep((end - now).min(Duration::from_millis(30)));
        }
    };

    let templates: Vec<Template> = p
        .images
        .iter()
        .filter_map(|path| match Template::load(path) {
            Ok(t) => Some(t),
            Err(e) => {
                emit(Event::Log(format!("Error: {e:#}")));
                None
            }
        })
        .collect();
    if templates.is_empty() {
        emit(Event::Log("No valid images to search for.".into()));
        return Ok(());
    }

    emit(Event::Log("Starting in 1 second. Press Esc to stop.".into()));
    sleep(START_DELAY)?;

    for i in 1..=p.max_loops {
        emit(Event::Iteration(i));
        for t in &templates {
            check()?;
            emit(Event::Status(format!("Searching for {}", t.name)));
            let started = Instant::now();
            match find_on_screens(t) {
                Ok(Some(hit)) if hit.score >= p.threshold && hit.contrast < MIN_CONTRAST => {
                    emit(Event::Log(format!(
                        "#{i} {}: found ({:.0}%) but dimmed (contrast {:.0}%), probably behind a dialog, skipping",
                        t.name,
                        hit.score * 100.0,
                        hit.contrast * 100.0
                    )))
                }
                Ok(Some(hit)) if hit.score >= p.threshold => {
                    emit(Event::Log(format!(
                        "#{i} {}: found ({:.0}%, {} ms), clicking at ({}, {})",
                        t.name,
                        hit.score * 100.0,
                        started.elapsed().as_millis(),
                        hit.x,
                        hit.y
                    )));
                    let should_stop = || check().is_err();
                    if input::move_smooth(hit.x, hit.y, MOVE_DURATION, &should_stop) {
                        input::left_click();
                    }
                    // The page changes after a click, so the remaining images
                    // can't be on screen anymore: end this iteration early.
                    sleep(p.click_interval)?;
                    break;
                }
                Ok(Some(hit)) => emit(Event::Log(format!(
                    "#{i} {}: not found (best match {:.0}%)",
                    t.name,
                    hit.score * 100.0
                ))),
                Ok(None) => emit(Event::Log(format!(
                    "#{i} {}: image is larger than the screen",
                    t.name
                ))),
                Err(e) => emit(Event::Log(format!("#{i} screen capture failed: {e:#}"))),
            }
            sleep(p.click_interval)?;
        }
        emit(Event::Status("Waiting for the next iteration".into()));
        sleep(p.loop_interval)?;
    }
    Ok(())
}

struct Hit {
    /// Center of the match, in virtual-desktop coordinates.
    x: i32,
    y: i32,
    score: f32,
    contrast: f32,
}

/// Captures every monitor and returns the best match across all of them.
fn find_on_screens(t: &Template) -> anyhow::Result<Option<Hit>> {
    let mut best: Option<Hit> = None;
    for monitor in xcap::Monitor::all()? {
        let shot = monitor.capture_image()?;
        let gray = Gray::from_rgba(&shot);
        if let Some(m) = t.find(&gray)
            && best.as_ref().is_none_or(|b| m.score > b.score)
        {
            best = Some(Hit {
                x: monitor.x()? + (m.x + t.width / 2) as i32,
                y: monitor.y()? + (m.y + t.height / 2) as i32,
                score: m.score,
                contrast: m.contrast,
            });
        }
    }
    Ok(best)
}
