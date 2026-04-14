use std::time::Instant;

#[derive(Debug, Clone)]
pub struct LoadingState {
  pub is_loading: bool,
  pub start_time: Instant,
  pub is_initial_load: bool,
  pub finish_time: Option<Instant>,
  pub updated_feeds: Vec<String>,
}

impl LoadingState {
  pub fn new() -> Self {
    Self {
      is_loading: true,
      is_initial_load: true,
      start_time: Instant::now(),
      finish_time: None,
      updated_feeds: Vec::new(),
    }
  }

  /// Create a loading state that starts in the idle (not loading) position.
  pub fn idle() -> Self {
    let mut state = Self::new();
    state.stop();
    state
  }

  pub fn start(&mut self) {
    self.is_loading = true;
    self.is_initial_load = false;
    self.start_time = Instant::now();
    self.finish_time = None;
    self.updated_feeds.clear();
  }

  pub fn stop(&mut self) {
    self.is_loading = false;
    self.finish_time = Some(Instant::now());
  }

  pub fn elapsed_secs(&self) -> u64 {
    self.start_time.elapsed().as_secs()
  }

  /// Returns true while loading, and for 3 seconds after loading finishes.
  pub fn should_show_popup(&self) -> bool {
    if self.is_loading {
      return true;
    }
    if let Some(finish) = self.finish_time {
      return finish.elapsed().as_secs() < 3;
    }
    false
  }

  pub fn spinner_frame(&self) -> &'static str {
    if !self.is_loading {
      return "";
    }
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let index = (self.start_time.elapsed().as_millis() / 80) as usize % frames.len();
    frames[index]
  }
}
