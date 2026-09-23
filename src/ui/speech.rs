//! Speech through Prism, or the console when Prism isn't there or
//! FF_DISPATCH_NO_SPEECH=1.

pub const NO_SPEECH_ENV: &str = "FF_DISPATCH_NO_SPEECH";

pub trait Speaker {
    fn say(&mut self, text: &str, interrupt: bool);
    fn stop(&mut self);
}

/// Prints each line (when `echo`) and keeps them (for tests and scripts).
#[derive(Default)]
pub struct Console {
    pub lines: Vec<String>,
    pub echo: bool,
}

impl Speaker for Console {
    fn say(&mut self, text: &str, _interrupt: bool) {
        if self.echo {
            println!("{text}");
        }
        self.lines.push(text.to_string());
    }

    fn stop(&mut self) {}
}

struct Prism {
    backend: prismer::Backend<'static>,
}

impl Speaker for Prism {
    fn say(&mut self, text: &str, interrupt: bool) {
        if text.trim().is_empty() {
            return;
        }
        if self.backend.output(text, interrupt).is_err() {
            let _ = self.backend.speak(text, interrupt);
        }
    }

    fn stop(&mut self) {
        let _ = self.backend.stop();
    }
}

pub fn speaker() -> Box<dyn Speaker> {
    if std::env::var(NO_SPEECH_ENV).is_ok_and(|v| v == "1") {
        return Box::new(Console {
            lines: Vec::new(),
            echo: true,
        });
    }
    // ponytail: one Prism context per process, leaked so its backend can be
    // 'static; add backend refresh (as FF does) if screen-reader switching
    // mid-session matters to testers.
    let backend = prismer::Prism::new().ok().and_then(|p| {
        let prism: &'static prismer::Prism = Box::leak(Box::new(p));
        let backend = prism.acquire_best().ok()?;
        match backend.initialize() {
            Ok(()) | Err(prismer::Error::AlreadyInitialized) => Some(backend),
            Err(_) => None,
        }
    });
    match backend {
        Some(backend) => Box::new(Prism { backend }),
        None => {
            log::warn!("Prism unavailable; speech goes to the console");
            Box::new(Console {
                lines: Vec::new(),
                echo: true,
            })
        }
    }
}
