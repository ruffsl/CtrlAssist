// src/tui/event.rs

use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::app::{FocusedSection, TuiApp};

pub fn handle_events(app: &mut TuiApp) -> Result<(), Box<dyn std::error::Error>> {
    if event::poll(Duration::from_millis(100))? {
        if let CrosstermEvent::Key(key) = event::read()? {
            handle_key_event(app, key);
        }
    }
    Ok(())
}

fn handle_key_event(app: &mut TuiApp, key: KeyEvent) {
    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        // Navigation
        KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
            app.focused_section = app.focused_section.next();
        }
        KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
            app.focused_section = app.focused_section.prev();
        }

        // Actions
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
            handle_action(app);
        }

        // Refresh controllers
        KeyCode::Char('r') => {
            if let Err(e) = app.refresh_controllers() {
                app.status_message = format!("Refresh failed: {}", e);
            } else {
                app.status_message = "Controllers refreshed".to_string();
            }
        }

        _ => {}
    }
}

fn handle_action(app: &mut TuiApp) {
    match app.focused_section {
        FocusedSection::PrimaryController => app.cycle_primary(),
        FocusedSection::AssistController => app.cycle_assist(),
        FocusedSection::Mode => app.cycle_mode(),
        FocusedSection::Hide => app.cycle_hide(),
        FocusedSection::Spoof => app.cycle_spoof(),
        FocusedSection::Rumble => app.cycle_rumble(),
        FocusedSection::StartStop => {
            if app.status == crate::tui::app::MuxStatus::Stopped {
                app.start_mux();
            } else {
                app.stop_mux();
            }
        }
    }
}
