// src/tui/ui.rs

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::app::{FocusedSection, MuxStatus, TuiApp};

pub fn draw(f: &mut Frame, app: &TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(0),     // Main content
            Constraint::Length(3),  // Status bar
        ])
        .split(f.area());

    draw_title(f, chunks[0]);
    draw_main_content(f, app, chunks[1]);
    draw_status_bar(f, app, chunks[2]);
}

fn draw_title(f: &mut Frame, area: Rect) {
    let title = Paragraph::new("CtrlAssist TUI")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
}

fn draw_main_content(f: &mut Frame, app: &TuiApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Controllers
            Constraint::Length(11), // Settings
            Constraint::Length(3),  // Start/Stop
            Constraint::Min(0),     // Help
        ])
        .split(area);

    draw_controllers(f, app, chunks[0]);
    draw_settings(f, app, chunks[1]);
    draw_start_stop(f, app, chunks[2]);
    draw_help(f, chunks[3]);
}

fn draw_controllers(f: &mut Frame, app: &TuiApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Primary Controller
    let is_focused = app.focused_section == FocusedSection::PrimaryController;
    let primary_block = Block::default()
        .title(format!(
            "Primary Controller {}",
            if is_focused { "[ENTER to change]" } else { "" }
        ))
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });

    let primary_text = if app.controllers.is_empty() {
        "No controllers found".to_string()
    } else {
        format!(
            "({}) {}",
            app.selected_primary
                .map(|id| id.to_string())
                .unwrap_or_else(|| "#".to_string()),
            truncate_name(&app.get_primary_name(), 25)
        )
    };

    let primary = Paragraph::new(primary_text)
        .block(primary_block)
        .alignment(Alignment::Center)
        .style(if app.status == MuxStatus::Running {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        });
    f.render_widget(primary, chunks[0]);

    // Assist Controller
    let is_focused = app.focused_section == FocusedSection::AssistController;
    let assist_block = Block::default()
        .title(format!(
            "Assist Controller {}",
            if is_focused { "[ENTER to change]" } else { "" }
        ))
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });

    let assist_text = if app.controllers.is_empty() {
        "No controllers found".to_string()
    } else {
        format!(
            "({}) {}",
            app.selected_assist
                .map(|id| id.to_string())
                .unwrap_or_else(|| "#".to_string()),
            truncate_name(&app.get_assist_name(), 25)
        )
    };

    let assist = Paragraph::new(assist_text)
        .block(assist_block)
        .alignment(Alignment::Center)
        .style(if app.status == MuxStatus::Running {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        });
    f.render_widget(assist, chunks[1]);
}

fn draw_settings(f: &mut Frame, app: &TuiApp, area: Rect) {
    let is_running = app.status == MuxStatus::Running;

    let items: Vec<ListItem> = vec![
        create_setting_item(
            "Mode",
            &format!("{:?}", app.mode),
            app.focused_section == FocusedSection::Mode,
            true, // Always changeable
        ),
        create_setting_item(
            "Hide",
            &format!("{:?}", app.hide),
            app.focused_section == FocusedSection::Hide,
            !is_running,
        ),
        create_setting_item(
            "Spoof",
            &format!("{:?}", app.spoof),
            app.focused_section == FocusedSection::Spoof,
            !is_running,
        ),
        create_setting_item(
            "Rumble",
            &format!("{:?}", app.rumble),
            app.focused_section == FocusedSection::Rumble,
            true, // Always changeable
        ),
    ];

    let settings_list = List::new(items)
        .block(
            Block::default()
                .title("Settings [ENTER to cycle]")
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(settings_list, area);
}

fn create_setting_item(
    label: &str,
    value: &str,
    is_focused: bool,
    is_enabled: bool,
) -> ListItem<'static> {
    let style = if !is_enabled {
        Style::default().fg(Color::DarkGray)
    } else if is_focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let prefix = if is_focused { "▶ " } else { "  " };
    let text = format!("{}{:12} : {}", prefix, label, value);

    ListItem::new(text).style(style)
}

fn draw_start_stop(f: &mut Frame, app: &TuiApp, area: Rect) {
    let is_focused = app.focused_section == FocusedSection::StartStop;
    let is_valid = app.is_valid_for_start();

    let (text, style) = match app.status {
        MuxStatus::Stopped => {
            if is_valid {
                ("Press ENTER to Start Mux", Style::default().fg(Color::Green))
            } else {
                (
                    "Select two different controllers to start",
                    Style::default().fg(Color::Red),
                )
            }
        }
        MuxStatus::Running => ("Press ENTER to Stop Mux", Style::default().fg(Color::Yellow)),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        });

    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center)
        .style(style);

    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, app: &TuiApp, area: Rect) {
    let status_text = format!(
        "Status: {} | {}",
        match app.status {
            MuxStatus::Stopped => "Stopped",
            MuxStatus::Running => "Running",
        },
        app.status_message
    );

    let status = Paragraph::new(status_text)
        .style(match app.status {
            MuxStatus::Stopped => Style::default().fg(Color::Gray),
            MuxStatus::Running => Style::default().fg(Color::Green),
        })
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(status, area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(vec![
            Span::styled("Navigation: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("Tab/↓/j: Next | Shift+Tab/↑/k: Previous"),
        ]),
        Line::from(vec![
            Span::styled("Actions: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("Enter/Space/→/l: Select/Cycle | r: Refresh"),
        ]),
        Line::from(vec![
            Span::styled("Quit: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("q/Esc/Ctrl+c"),
        ]),
    ];

    let help = Paragraph::new(help_text)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .style(Style::default().fg(Color::Cyan));

    f.render_widget(help, area);
}

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() > max_len {
        format!("{}...", &name[..max_len - 3])
    } else {
        name.to_string()
    }
}
