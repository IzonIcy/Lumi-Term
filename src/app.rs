use crate::{
    chrome_bridge::{ChromeBridge, OpacityProfile},
    config::AppConfig,
    pty::{CellStyle, StyledSpan, TerminalSession, TerminalSnapshot},
};
use anyhow::{Context, Result};
use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, Sense, Shadow,
    Stroke,
};
use std::time::Duration;
use vt100::Color;

const MIN_TERM_COLS: u16 = 2;
const MIN_TERM_ROWS: u16 = 2;

#[derive(Clone, Copy)]
struct CellMetrics {
    width: f32,
    height: f32,
}

#[derive(Clone, Copy)]
struct ChromePalette {
    desktop_bg: Color32,
    window_bg: Color32,
    window_border: Color32,
    titlebar_bg: Color32,
    tabbar_bg: Color32,
    terminal_bg: Color32,
    terminal_border: Color32,
    text_primary: Color32,
    text_muted: Color32,
}

#[derive(Clone, Copy, Default)]
struct TrafficLightAction {
    close: bool,
    minimize: bool,
    maximize: bool,
}

struct TerminalTab {
    title: String,
    follow_output: bool,
    session: TerminalSession,
    snapshot: TerminalSnapshot,
    session_closed: bool,
}

impl Default for ChromePalette {
    fn default() -> Self {
        Self::from_opacity_profile(ChromeBridge::default_opacity_profile())
    }
}

impl ChromePalette {
    fn from_opacity_profile(profile: OpacityProfile) -> Self {
        Self {
            desktop_bg: Color32::from_rgba_unmultiplied(8, 10, 16, profile.desktop_alpha),
            window_bg: Color32::from_rgba_unmultiplied(13, 16, 24, profile.window_alpha),
            window_border: Color32::from_rgba_unmultiplied(
                144,
                153,
                176,
                profile.window_border_alpha,
            ),
            titlebar_bg: Color32::from_rgba_unmultiplied(24, 28, 39, profile.titlebar_alpha),
            tabbar_bg: Color32::from_rgba_unmultiplied(31, 35, 48, profile.tabbar_alpha),
            terminal_bg: Color32::from_rgba_unmultiplied(8, 11, 18, profile.terminal_alpha),
            terminal_border: Color32::from_rgba_unmultiplied(
                108,
                118,
                146,
                profile.terminal_border_alpha,
            ),
            text_primary: Color32::from_rgb(232, 236, 246),
            text_muted: Color32::from_rgb(156, 162, 178),
        }
    }
}

pub struct LumiTermApp {
    config: AppConfig,
    tabs: Vec<TerminalTab>,
    chrome: ChromeBridge,
    status_message: Option<String>,
    session_title: String,
    rows: u16,
    cols: u16,
}

impl LumiTermApp {
    pub fn new(config: AppConfig) -> Result<Self> {
        let (rows, cols) = estimate_grid_size(
            config.window.width,
            config.window.height,
            fallback_cell_metrics(config.terminal.font_size),
        );

        let first_session = TerminalSession::new(rows, cols, &config.terminal)
            .context("creating initial terminal session")?;
        let first_snapshot = first_session.snapshot();
        let tabs = vec![TerminalTab {
            title: "Shell 1".to_owned(),
            follow_output: true,
            session: first_session,
            snapshot: first_snapshot,
            session_closed: false,
        }];

        Ok(Self {
            config,
            tabs,
            chrome: ChromeBridge::new(1),
            status_message: None,
            session_title: format_session_title(),
            rows,
            cols,
        })
    }

    pub fn error(title: String, message: String) -> Self {
        let config = AppConfig {
            window: crate::config::WindowConfig {
                title,
                width: 900.0,
                height: 520.0,
            },
            ..AppConfig::default()
        };

        Self {
            config,
            tabs: Vec::new(),
            chrome: ChromeBridge::new(0),
            status_message: Some(message),
            session_title: "Lumi-Term".to_owned(),
            rows: MIN_TERM_ROWS,
            cols: MIN_TERM_COLS,
        }
    }

    fn active_tab(&self) -> Option<&TerminalTab> {
        self.tabs.get(self.chrome.active_tab())
    }

    fn active_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let active = self.chrome.active_tab();
        self.tabs.get_mut(active)
    }

    fn create_tab(&mut self, title: impl Into<String>) {
        match self.build_tab(title) {
            Ok(tab) => {
                self.tabs.push(tab);
                self.chrome.set_tab_count(self.tabs.len());
                self.chrome
                    .set_active_tab(self.tabs.len().saturating_sub(1));
            }
            Err(error) => {
                self.status_message = Some(format!("new tab failed: {error}"));
            }
        }
    }

    fn build_tab(&self, title: impl Into<String>) -> Result<TerminalTab> {
        let session = TerminalSession::new(self.rows, self.cols, &self.config.terminal)
            .context("creating terminal session")?;
        let snapshot = session.snapshot();
        Ok(TerminalTab {
            title: title.into(),
            follow_output: true,
            session,
            snapshot,
            session_closed: false,
        })
    }

    fn restart_active_tab(&mut self) {
        let active = self.chrome.active_tab();
        if let Some(tab) = self.tabs.get_mut(active) {
            match TerminalSession::new(self.rows, self.cols, &self.config.terminal) {
                Ok(session) => {
                    let snapshot = session.snapshot();
                    tab.session = session;
                    tab.snapshot = snapshot;
                    tab.follow_output = true;
                    tab.session_closed = false;
                    self.status_message = None;
                }
                Err(error) => {
                    self.status_message = Some(format!("restart failed: {error}"));
                }
            }
        }
    }

    fn select_next_tab(&mut self) {
        self.chrome.set_tab_count(self.tabs.len());
        self.chrome.next_tab();
    }

    fn select_previous_tab(&mut self) {
        self.chrome.set_tab_count(self.tabs.len());
        self.chrome.previous_tab();
    }

    fn ingest_events(&mut self, ctx: &egui::Context) {
        let scroll_delta = ctx.input(|input| input.smooth_scroll_delta.y);
        let wheel_lines = (scroll_delta / self.config.terminal.font_size.max(1.0)).round() as i32;
        if wheel_lines != 0
            && let Some(tab) = self.active_tab_mut()
        {
            tab.session.scroll_by_lines(wheel_lines);
            tab.follow_output = false;
        }

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                egui::Event::Copy => {
                    self.copy_visible_text(ctx);
                }
                egui::Event::Text(text) => {
                    let filtered: String = text.chars().filter(|char| !char.is_control()).collect();
                    if !filtered.is_empty() {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.follow_output = true;
                        }
                        self.send_text(&filtered);
                    }
                }
                egui::Event::Paste(text) => {
                    if let Some(tab) = self.active_tab_mut() {
                        tab.follow_output = true;
                    }
                    self.send_text(&text);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if key == egui::Key::PageUp && modifiers.shift {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.session.scroll_by_lines(10);
                            tab.follow_output = false;
                        }
                        continue;
                    }
                    if key == egui::Key::PageDown && modifiers.shift {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.session.scroll_by_lines(-10);
                            tab.follow_output = false;
                        }
                        continue;
                    }
                    if key == egui::Key::End && modifiers.ctrl {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.session.jump_to_live_output();
                            tab.follow_output = true;
                        }
                        continue;
                    }
                    if key == egui::Key::C
                        && modifiers.shift
                        && (modifiers.command || modifiers.ctrl)
                    {
                        self.copy_visible_text(ctx);
                        continue;
                    }

                    if let Some(bytes) = map_key_to_bytes(key, modifiers) {
                        if let Some(tab) = self.active_tab_mut() {
                            tab.follow_output = true;
                        }
                        self.send_bytes(&bytes);
                    }
                }
                _ => {}
            }
        }
    }

    fn send_text(&mut self, text: &str) {
        if let Some(tab) = self.active_tab_mut()
            && let Err(error) = tab.session.send_text(text)
        {
            self.status_message = Some(format!("input error: {error}"));
        }
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        if let Some(tab) = self.active_tab_mut()
            && let Err(error) = tab.session.send_bytes(bytes)
        {
            self.status_message = Some(format!("input error: {error}"));
        }
    }

    fn sync_size(&mut self, width: f32, height: f32, ctx: &egui::Context) {
        let metrics = cell_metrics_from_context(ctx, self.config.terminal.font_size);
        let (rows, cols) = estimate_grid_size(width, height, metrics);
        if rows == self.rows && cols == self.cols {
            return;
        }

        self.rows = rows;
        self.cols = cols;
        for tab in &mut self.tabs {
            if let Err(error) = tab.session.resize(rows, cols) {
                self.status_message = Some(format!("resize error: {error}"));
                break;
            }
            tab.snapshot = tab.session.snapshot();
        }
    }

    fn draw_titlebar(&mut self, ui: &mut egui::Ui, palette: ChromePalette, ctx: &egui::Context) {
        Frame::new()
            .fill(palette.titlebar_bg)
            .stroke(Stroke::new(1.0, palette.window_border))
            .corner_radius(CornerRadius {
                nw: 16,
                ne: 16,
                sw: 0,
                se: 0,
            })
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let traffic = draw_traffic_lights(ui);
                    if traffic.close {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if traffic.minimize {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    if traffic.maximize {
                        let maximized = self.chrome.toggle_maximized();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(maximized));
                    }

                    ui.add_space(8.0);
                    if draw_icon_button(ui, "+", palette) {
                        self.create_tab("Shell");
                    }
                    if draw_icon_button(ui, "⇱", palette) {
                        self.status_message = Some("Split panes are not available yet.".to_owned());
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Lumi-Term")
                            .size(14.2)
                            .strong()
                            .color(palette.text_primary),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if draw_icon_button(ui, "≡", palette) {
                            self.status_message = Some("Main menu actions coming soon.".to_owned());
                        }
                        if draw_icon_button(ui, "▦", palette) {
                            self.select_next_tab();
                        }
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(&self.session_title)
                                .size(12.2)
                                .monospace()
                                .color(palette.text_muted),
                        );
                    });
                });
            });
    }

    fn draw_tabbar(&mut self, ui: &mut egui::Ui, palette: ChromePalette) {
        self.chrome.set_tab_count(self.tabs.len());
        Frame::new()
            .fill(palette.tabbar_bg)
            .stroke(Stroke::new(1.0, palette.window_border.gamma_multiply(0.6)))
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if draw_icon_button(ui, "＋", palette) {
                        self.create_tab("Shell");
                    }
                    ui.add_space(6.0);

                    let mut clicked_tab: Option<usize> = None;
                    for (index, tab) in self.tabs.iter().enumerate() {
                        if draw_tab_chip(
                            ui,
                            &(index + 1).to_string(),
                            &tab.title,
                            index == self.chrome.active_tab(),
                            palette,
                        ) {
                            clicked_tab = Some(index);
                        }
                        ui.add_space(6.0);
                    }

                    if let Some(index) = clicked_tab {
                        self.chrome.set_active_tab(index);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if draw_icon_button(ui, "⋯", palette) {
                            self.status_message = Some("Tab context menu coming soon.".to_owned());
                        }
                        if draw_icon_button(ui, "▣", palette) {
                            self.select_previous_tab();
                        }
                    });
                });
            });
    }

    fn draw_terminal_surface(
        &mut self,
        ui: &mut egui::Ui,
        palette: ChromePalette,
        ctx: &egui::Context,
    ) {
        let theme_bg = Color32::from_rgba_unmultiplied(
            self.config.theme.background[0],
            self.config.theme.background[1],
            self.config.theme.background[2],
            84,
        );
        let theme_fg = Color32::from_rgb(
            self.config.theme.foreground[0],
            self.config.theme.foreground[1],
            self.config.theme.foreground[2],
        );

        Frame::new()
            .fill(palette.terminal_bg)
            .stroke(Stroke::new(1.0, palette.terminal_border))
            .corner_radius(CornerRadius {
                nw: 0,
                ne: 0,
                sw: 16,
                se: 16,
            })
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                let is_scrollback = self
                    .active_tab()
                    .is_some_and(|tab| tab.snapshot.at_scrollback_top);
                if let Some(tab) = self.active_tab_mut() {
                    tab.follow_output = tab.follow_output && !is_scrollback;
                }

                if is_scrollback {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(132, 176, 255, 36))
                            .stroke(Stroke::new(
                                1.0,
                                Color32::from_rgba_unmultiplied(132, 176, 255, 80),
                            ))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(8, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("SCROLLBACK")
                                        .size(11.5)
                                        .strong()
                                        .color(Color32::from_rgb(162, 198, 255)),
                                );
                            });
                    });
                    ui.add_space(3.0);
                }

                let terminal_size = ui.available_size();
                self.sync_size(terminal_size.x, terminal_size.y, ctx);

                ui.allocate_ui_with_layout(terminal_size, Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    if let Some(tab) = self.active_tab() {
                        let font_id = egui::FontId::monospace(self.config.terminal.font_size);
                        for row in &tab.snapshot.rows {
                            let mut layout_job = egui::text::LayoutJob::default();
                            for span in row {
                                append_span(&mut layout_job, span, &font_id, theme_fg, theme_bg);
                            }
                            ui.label(layout_job);
                        }
                    } else {
                        ui.label(
                            RichText::new("Lumi-Term failed to start")
                                .size(14.0)
                                .color(theme_fg),
                        );
                    }
                });

                if self.active_tab().is_some_and(|tab| tab.session_closed) {
                    ui.add_space(8.0);
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Session ended")
                                .size(12.8)
                                .strong()
                                .color(Color32::from_rgb(230, 180, 180)),
                        );
                        if ui.button("Restart session").clicked() {
                            self.restart_active_tab();
                        }
                    });
                }
            });
    }

    fn copy_visible_text(&self, ctx: &egui::Context) {
        if let Some(tab) = self.active_tab() {
            let text = snapshot_to_plain_text(&tab.snapshot);
            ctx.copy_text(text);
        }
    }
}

impl eframe::App for LumiTermApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ingest_events(&ctx);

        if let Some(tab) = self.active_tab_mut() {
            let has_updates = tab.session.poll_output();
            if has_updates {
                if tab.follow_output {
                    tab.session.jump_to_live_output();
                }
                tab.snapshot = tab.session.snapshot();
                ctx.request_repaint();
            }
            if tab.session.is_closed() && !tab.session_closed {
                tab.session_closed = true;
                tab.follow_output = false;
                self.status_message = Some("Session ended. Restart to continue.".to_owned());
            }
        }

        let palette = ChromePalette::default();
        let app_rect = ui.max_rect();
        ui.painter().rect_filled(app_rect, 0.0, palette.desktop_bg);

        Frame::new()
            .fill(palette.window_bg)
            .stroke(Stroke::new(1.0, palette.window_border))
            .corner_radius(CornerRadius::same(18))
            .shadow(Shadow {
                offset: [0, 10],
                blur: 32,
                spread: 0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 42),
            })
            .outer_margin(Margin::same(8))
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                self.draw_titlebar(ui, palette, &ctx);
                self.draw_tabbar(ui, palette);
                self.draw_terminal_surface(ui, palette, &ctx);

                if let Some(status) = &self.status_message {
                    Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(255, 112, 112, 22))
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(status)
                                    .size(12.5)
                                    .color(Color32::from_rgb(255, 150, 150)),
                            );
                        });
                }
            });

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn append_span(
    job: &mut egui::text::LayoutJob,
    span: &StyledSpan,
    font_id: &egui::FontId,
    theme_fg: Color32,
    theme_bg: Color32,
) {
    let (fg, bg) = resolve_colors(span.style, theme_fg, theme_bg);
    let mut format = egui::TextFormat {
        font_id: font_id.clone(),
        color: fg,
        background: bg,
        ..Default::default()
    };

    if span.style.italic {
        format.italics = true;
    }
    if span.style.underline {
        format.underline = Stroke::new(1.0, fg);
    }

    job.append(&span.text, 0.0, format);
}

fn draw_tab_chip(
    ui: &mut egui::Ui,
    index: &str,
    title: &str,
    active: bool,
    palette: ChromePalette,
) -> bool {
    let stroke = if active {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 52))
    } else {
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18))
    };

    let fill = if active {
        Color32::from_rgba_unmultiplied(108, 116, 141, 128)
    } else {
        Color32::from_rgba_unmultiplied(40, 45, 60, 82)
    };

    let text = format!("#{index}  {title}");
    ui.add(
        Button::new(RichText::new(text).size(12.6).strong().color(if active {
            palette.text_primary
        } else {
            palette.text_muted
        }))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(9))
        .min_size(egui::vec2(90.0, 26.0)),
    )
    .clicked()
}

fn draw_icon_button(ui: &mut egui::Ui, icon: &str, palette: ChromePalette) -> bool {
    ui.add(
        Button::new(
            RichText::new(icon)
                .size(12.6)
                .strong()
                .color(palette.text_primary),
        )
        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 18))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 22),
        ))
        .corner_radius(CornerRadius::same(8))
        .min_size(egui::vec2(24.0, 22.0)),
    )
    .clicked()
}

fn draw_traffic_lights(ui: &mut egui::Ui) -> TrafficLightAction {
    let mut actions = TrafficLightAction::default();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 14.0), Sense::hover());
    let y = rect.center().y;
    let radius = 5.0;
    let spacing = 14.0;
    let start_x = rect.left() + radius + 1.0;
    let colors = [
        Color32::from_rgb(255, 95, 87),
        Color32::from_rgb(255, 189, 46),
        Color32::from_rgb(41, 203, 65),
    ];

    for (index, color) in colors.into_iter().enumerate() {
        let x = start_x + (index as f32 * spacing);
        let center = egui::pos2(x, y);
        let hitbox = egui::Rect::from_center_size(center, egui::vec2(12.0, 12.0));
        let response = ui.interact(
            hitbox,
            ui.make_persistent_id(("traffic-light", index)),
            Sense::click(),
        );
        let paint_color = if response.hovered() {
            color.gamma_multiply(1.15)
        } else {
            color
        };
        ui.painter().circle_filled(center, radius, paint_color);

        if response.clicked() {
            match index {
                0 => actions.close = true,
                1 => actions.minimize = true,
                2 => actions.maximize = true,
                _ => {}
            }
        }
    }

    actions
}

fn resolve_colors(style: CellStyle, theme_fg: Color32, theme_bg: Color32) -> (Color32, Color32) {
    let mut fg = resolve_vt_color(style.fg, theme_fg);
    let mut bg = resolve_vt_color(style.bg, theme_bg);

    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.bold {
        fg = fg.gamma_multiply(1.18);
    }
    if style.dim {
        fg = fg.gamma_multiply(0.72);
    }

    (fg, bg)
}

fn resolve_vt_color(color: Color, fallback: Color32) -> Color32 {
    match color {
        Color::Default => fallback,
        Color::Idx(idx) => xterm_index_to_rgb(idx),
        Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

fn xterm_index_to_rgb(index: u8) -> Color32 {
    const ANSI_16: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];

    if index < 16 {
        let [r, g, b] = ANSI_16[index as usize];
        return Color32::from_rgb(r, g, b);
    }

    if (16..=231).contains(&index) {
        let color_index = index - 16;
        let r = color_index / 36;
        let g = (color_index % 36) / 6;
        let b = color_index % 6;
        let map = [0_u8, 95, 135, 175, 215, 255];
        return Color32::from_rgb(map[r as usize], map[g as usize], map[b as usize]);
    }

    let gray = 8_u8.saturating_add((index - 232).saturating_mul(10));
    Color32::from_rgb(gray, gray, gray)
}

fn estimate_grid_size(width: f32, height: f32, metrics: CellMetrics) -> (u16, u16) {
    let cell_width = metrics.width.max(1.0);
    let cell_height = metrics.height.max(1.0);
    let cols = (width / cell_width).floor() as u16;
    let rows = (height / cell_height).floor() as u16;
    (rows.max(MIN_TERM_ROWS), cols.max(MIN_TERM_COLS))
}

fn fallback_cell_metrics(font_size: f32) -> CellMetrics {
    CellMetrics {
        width: font_size * 0.60,
        height: font_size * 1.32,
    }
}

fn cell_metrics_from_context(ctx: &egui::Context, font_size: f32) -> CellMetrics {
    let font_id = egui::FontId::monospace(font_size);
    let (width, height) =
        ctx.fonts_mut(|fonts| (fonts.glyph_width(&font_id, 'W'), fonts.row_height(&font_id)));
    let metrics = CellMetrics { width, height };
    if metrics.width <= 0.0 || metrics.height <= 0.0 {
        fallback_cell_metrics(font_size)
    } else {
        metrics
    }
}

fn map_key_to_bytes(key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
    if modifiers.ctrl
        && let Some(control_byte) = ctrl_key_to_byte(key)
    {
        return Some(vec![control_byte]);
    }

    if modifiers.alt {
        if let Some(sequence) = alt_modified_key_sequence(key, modifiers.shift) {
            return Some(sequence);
        }
    }

    let bytes: &[u8] = match key {
        egui::Key::Enter => b"\r",
        egui::Key::Backspace => b"\x7f",
        egui::Key::Tab if modifiers.shift => b"\x1b[Z",
        egui::Key::Tab => b"\t",
        egui::Key::Escape => b"\x1b",
        egui::Key::ArrowUp => b"\x1b[A",
        egui::Key::ArrowDown => b"\x1b[B",
        egui::Key::ArrowRight => b"\x1b[C",
        egui::Key::ArrowLeft => b"\x1b[D",
        egui::Key::Home => b"\x1b[H",
        egui::Key::End => b"\x1b[F",
        egui::Key::Insert => b"\x1b[2~",
        egui::Key::Delete => b"\x1b[3~",
        egui::Key::PageUp => b"\x1b[5~",
        egui::Key::PageDown => b"\x1b[6~",
        egui::Key::F1 => b"\x1bOP",
        egui::Key::F2 => b"\x1bOQ",
        egui::Key::F3 => b"\x1bOR",
        egui::Key::F4 => b"\x1bOS",
        egui::Key::F5 => b"\x1b[15~",
        egui::Key::F6 => b"\x1b[17~",
        egui::Key::F7 => b"\x1b[18~",
        egui::Key::F8 => b"\x1b[19~",
        egui::Key::F9 => b"\x1b[20~",
        egui::Key::F10 => b"\x1b[21~",
        egui::Key::F11 => b"\x1b[23~",
        egui::Key::F12 => b"\x1b[24~",
        _ => return None,
    };

    Some(bytes.to_vec())
}

fn alt_modified_key_sequence(key: egui::Key, shift: bool) -> Option<Vec<u8>> {
    let modifier = if shift { 4 } else { 3 };
    let sequence = match key {
        egui::Key::ArrowUp => format!("\x1b[1;{modifier}A"),
        egui::Key::ArrowDown => format!("\x1b[1;{modifier}B"),
        egui::Key::ArrowRight => format!("\x1b[1;{modifier}C"),
        egui::Key::ArrowLeft => format!("\x1b[1;{modifier}D"),
        egui::Key::Home => format!("\x1b[1;{modifier}H"),
        egui::Key::End => format!("\x1b[1;{modifier}F"),
        egui::Key::Insert => format!("\x1b[2;{modifier}~"),
        egui::Key::Delete => format!("\x1b[3;{modifier}~"),
        egui::Key::PageUp => format!("\x1b[5;{modifier}~"),
        egui::Key::PageDown => format!("\x1b[6;{modifier}~"),
        egui::Key::F1 => format!("\x1b[1;{modifier}P"),
        egui::Key::F2 => format!("\x1b[1;{modifier}Q"),
        egui::Key::F3 => format!("\x1b[1;{modifier}R"),
        egui::Key::F4 => format!("\x1b[1;{modifier}S"),
        egui::Key::F5 => format!("\x1b[15;{modifier}~"),
        egui::Key::F6 => format!("\x1b[17;{modifier}~"),
        egui::Key::F7 => format!("\x1b[18;{modifier}~"),
        egui::Key::F8 => format!("\x1b[19;{modifier}~"),
        egui::Key::F9 => format!("\x1b[20;{modifier}~"),
        egui::Key::F10 => format!("\x1b[21;{modifier}~"),
        egui::Key::F11 => format!("\x1b[23;{modifier}~"),
        egui::Key::F12 => format!("\x1b[24;{modifier}~"),
        _ => return None,
    };

    Some(sequence.into_bytes())
}

fn ctrl_key_to_byte(key: egui::Key) -> Option<u8> {
    match key {
        egui::Key::A => Some(0x01),
        egui::Key::B => Some(0x02),
        egui::Key::C => Some(0x03),
        egui::Key::D => Some(0x04),
        egui::Key::E => Some(0x05),
        egui::Key::F => Some(0x06),
        egui::Key::G => Some(0x07),
        egui::Key::H => Some(0x08),
        egui::Key::I => Some(0x09),
        egui::Key::J => Some(0x0A),
        egui::Key::K => Some(0x0B),
        egui::Key::L => Some(0x0C),
        egui::Key::M => Some(0x0D),
        egui::Key::N => Some(0x0E),
        egui::Key::O => Some(0x0F),
        egui::Key::P => Some(0x10),
        egui::Key::Q => Some(0x11),
        egui::Key::R => Some(0x12),
        egui::Key::S => Some(0x13),
        egui::Key::T => Some(0x14),
        egui::Key::U => Some(0x15),
        egui::Key::V => Some(0x16),
        egui::Key::W => Some(0x17),
        egui::Key::X => Some(0x18),
        egui::Key::Y => Some(0x19),
        egui::Key::Z => Some(0x1A),
        _ => None,
    }
}

fn snapshot_to_plain_text(snapshot: &TerminalSnapshot) -> String {
    let mut lines = Vec::with_capacity(snapshot.rows.len());
    for row in &snapshot.rows {
        let mut line = String::new();
        for span in row {
            line.push_str(&span.text);
        }
        lines.push(line.trim_end_matches(' ').to_owned());
    }
    lines.join("\n")
}

fn format_session_title() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_owned());
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "lumi".to_owned());
    format!("{user}@{host}")
}
