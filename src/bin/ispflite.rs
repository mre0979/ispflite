// SPDX-License-Identifier: GPL-3.0-or-later
//! ISPFLite window: prefix area, data columns, and the command line.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use eframe::egui::{
    self, Align2, Color32, Event, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2,
    ViewportCommand,
};
use ispflite::highlight::{ColorCarry, COBOL_KEYWORD, COMMENT, DIRECTIVE, KEYWORD, NUMBER, STRING};
use ispflite::{
    cobol_outline, cobol_path, color_line, copy_name_links, definition_at, help_text, name_index,
    paragraph_calls, same_file, swapbar_fields, swapbar_window, variable_uses, EditorKey,
    EditorView, Focus, NextLineCursor, ParagraphCall, PendingClose, PointerTarget, ScrollDir,
    Session,
};

const FONT_SIZE: f32 = 16.0;
const ZOOM_MIN_STEP: i32 = -8;
const ZOOM_MAX_STEP: i32 = 12;
const PREFIX_COLS: usize = 8;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn tell(level: rfd::MessageLevel, message: &str) {
    rfd::MessageDialog::new()
        .set_title("ISPFLite")
        .set_description(message)
        .set_level(level)
        .show();
}

fn main() -> eframe::Result {
    let mut centered = false;
    let mut file = None;
    let mut session = Session::new();
    for arg in std::env::args().skip(1) {
        if arg == "--help" || arg == "-h" || arg == "/?" {
            tell(
                rfd::MessageLevel::Info,
                "Usage: ispflite [file]\n\
                 ispflite-cli [file]    line-oriented session\n\
                 \n\
                 Tab moves between the data, the command line, and the prefix.\n\
                 Left or Right Ctrl runs prefix commands and the command line.\n\
                 Enter moves to the next line. Options can change both.",
            );
            return Ok(());
        }
        if arg == "--center" {
            centered = true;
            continue;
        }
        if file.is_none() {
            file = Some(arg);
        }
    }
    if let Some(arg) = file {
        if let Err(err) = session.open_path(std::path::Path::new(&arg)) {
            tell(rfd::MessageLevel::Error, &err.to_string());
            std::process::exit(1);
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("ISPFLite"),
        centered,
        ..Default::default()
    };
    eframe::run_native(
        "ISPFLite",
        options,
        Box::new(|cc| Ok(Box::new(Window::new(cc, session)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnterKey {
    Enter,
    RightCtrl,
    LeftCtrl,
    BothCtrl,
}

impl EnterKey {
    const CHOICES: [Self; 4] = [Self::BothCtrl, Self::Enter, Self::RightCtrl, Self::LeftCtrl];

    fn label(self) -> &'static str {
        match self {
            Self::Enter => "Enter key",
            Self::RightCtrl => "Right Ctrl Key",
            Self::LeftCtrl => "Left Ctrl Key",
            Self::BothCtrl => "Left and Right Ctrl Key",
        }
    }
}

/// Syntax colors for the data area. Dark Blue keeps the editor's own palette.
/// Classic uses the 3270 colors: green text and numbers, red COBOL keywords,
/// turquoise comments, white strings, and the line-number blue for Top of
/// Data, COPY, and EXEC CICS.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorTheme {
    Current,
    Pcom,
}

impl ColorTheme {
    const CHOICES: [Self; 2] = [Self::Pcom, Self::Current];

    fn label(self) -> &'static str {
        match self {
            Self::Current => "Dark Blue",
            Self::Pcom => "Classic",
        }
    }
}

struct SyntaxColors {
    text: Color32,
    keyword: Color32,
    comment: Color32,
    string: Color32,
    number: Color32,
    background: Color32,
    current_line: Color32,
    sentinel: Color32,
    cols: Color32,
    cols_background: Color32,
    cobol_keyword: Color32,
    directive: Color32,
}

const LINE_NUMBER_COLOR: Color32 = Color32::from_rgb(130, 176, 255);

fn syntax_colors(theme: ColorTheme) -> SyntaxColors {
    match theme {
        ColorTheme::Current => SyntaxColors {
            text: Color32::from_rgb(236, 236, 236),
            keyword: Color32::from_rgb(130, 176, 255),
            comment: Color32::from_rgb(120, 160, 120),
            string: Color32::from_rgb(255, 186, 92),
            number: Color32::from_rgb(180, 220, 255),
            background: Color32::from_rgb(0, 0, 28),
            current_line: Color32::from_rgb(24, 40, 120),
            sentinel: Color32::from_rgb(150, 150, 170),
            cols: Color32::from_rgb(120, 210, 210),
            cols_background: Color32::from_rgb(0, 28, 48),
            cobol_keyword: Color32::from_rgb(130, 176, 255),
            directive: Color32::from_rgb(130, 176, 255),
        },
        ColorTheme::Pcom => SyntaxColors {
            text: Color32::from_rgb(0, 255, 0),
            keyword: Color32::from_rgb(255, 255, 255),
            comment: Color32::from_rgb(0, 255, 255),
            string: Color32::from_rgb(255, 255, 255),
            number: Color32::from_rgb(0, 255, 0),
            background: Color32::from_rgb(0, 0, 0),
            current_line: Color32::from_rgb(0, 40, 0),
            sentinel: LINE_NUMBER_COLOR,
            cols: Color32::from_rgb(0, 255, 255),
            cols_background: Color32::from_rgb(0, 0, 0),
            cobol_keyword: Color32::from_rgb(255, 0, 0),
            directive: LINE_NUMBER_COLOR,
        },
    }
}

/// Enter assigned to Ctrl runs when that key is released by itself.
fn release_runs_enter(held: bool, used: bool, still_down: bool) -> bool {
    held && !used && !still_down
}

fn is_attention_ctrl(setting: EnterKey, key: Key, physical: Option<Key>) -> bool {
    setting != EnterKey::Enter && is_attention(setting, key, physical)
}

fn is_attention(setting: EnterKey, key: Key, physical: Option<Key>) -> bool {
    let which = physical.unwrap_or(key);
    match setting {
        EnterKey::Enter => key == Key::Enter || physical == Some(Key::Enter),
        EnterKey::RightCtrl => which == Key::ControlRight,
        EnterKey::LeftCtrl => which == Key::ControlLeft,
        EnterKey::BothCtrl => which == Key::ControlLeft || which == Key::ControlRight,
    }
}

/// A Ctrl key chosen as Enter is no longer a shortcut modifier.
fn ctrl_modifies(setting: EnterKey, left: bool, right: bool, modifiers_ctrl: bool) -> bool {
    match setting {
        EnterKey::Enter => left || right || modifiers_ctrl,
        EnterKey::RightCtrl => left,
        EnterKey::LeftCtrl => right,
        EnterKey::BothCtrl => false,
    }
}

#[derive(Clone, Copy)]
struct OptionsDraft {
    enter_key: EnterKey,
    next_line_at: NextLineCursor,
    color_theme: ColorTheme,
    recent_limit: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    Close,
    End,
    Reopen,
    Options,
    Help,
    About,
    Recent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextZoom {
    Outline,
    Editor,
    Command,
}

struct Window {
    view: EditorView,
    hscroll: usize,
    top_row: usize,
    page_rows: usize,
    visible_cols: usize,
    scroll_grab: Option<f32>,
    close_prompt: bool,
    allow_close: bool,
    end_prompt: bool,
    end_closes_window: bool,
    reopen_prompt: bool,
    help_open: bool,
    options_open: bool,
    options_draft: Option<OptionsDraft>,
    about_open: bool,
    enter_key: EnterKey,
    next_line_at: NextLineCursor,
    color_theme: ColorTheme,
    left_ctrl: bool,
    right_ctrl: bool,
    /// A Ctrl key assigned as Enter is down.
    attention_held: bool,
    /// Another key was pressed while that Ctrl key was held.
    attention_used: bool,
    /// Places visited by Ctrl-clicking a paragraph or variable. `jump_at` is the current one.
    /// History is global across tabs.
    jumps: Vec<JumpPlace>,
    jump_at: usize,
    /// Alt is held, so menu mnemonics are underlined.
    alt_held: bool,
    /// Open this top-level menu on the next paint (Alt+letter).
    pending_menu: Option<char>,
    /// Top menu kept open for Alt or Left/Right (F/E/V/W/H).
    active_top_menu: Option<char>,
    /// Select the first menu item once Alt and Left/Right are released.
    menu_select_pending: bool,
    /// Focus the first item of the active menu during this menu bar paint.
    menu_descend: bool,
    /// EXIT asked for Save As before the tab can close.
    exit_after_save: bool,
    /// First SWAPBAR entry on screen, and whether the cursor is on that bar.
    swap_from: usize,
    swap_point: bool,
    /// Highlighted row in the SWAP LIST dialog. The last row is "Start a new screen".
    swap_list_at: Option<usize>,
    outline_width: f32,
    /// Width to restore when the outline is shown again after it was hidden.
    outline_restore: f32,
    /// Pointer x and panel width where the current divider drag began.
    outline_drag: Option<(f32, f32)>,
    /// Tab and paragraph last brought into the outline. A later manual scroll may hide it.
    outline_follow: Option<(u32, usize)>,
    /// The pointer is over the outline, so the wheel scrolls that panel.
    outline_hovered: bool,
    recent_limit: usize,
    recent: Vec<std::path::PathBuf>,
    recent_open: bool,
    /// Right-click menu for one file tab. The position is the pointer.
    tab_menu: Option<TabMenu>,
    /// Ignore the click that opened the tab menu.
    tab_menu_fresh: bool,
    /// Right-click menu for Go to definition. The position is the pointer.
    editor_menu: Option<EditorMenu>,
    /// Ignore the click that opened the editor menu.
    editor_menu_fresh: bool,
    /// Tab being dragged by the left button.
    tab_drag: Option<u32>,
    /// The icon row under the menu. Hidden from the View menu.
    show_toolbar: bool,
    /// The line and column bar. Hidden from the View menu.
    show_status_bar: bool,
    /// Text-size steps for the outline, the editor, and the command line.
    zoom_outline: i32,
    zoom_editor: i32,
    zoom_command: i32,
    /// Partial Ctrl+wheel movement, so one notch is one zoom step.
    zoom_wheel: f32,
    /// Area that accumulated the current Ctrl+wheel movement.
    zoom_wheel_part: Option<TextZoom>,
    crosshair: bool,
    /// Command line and editor rectangles, used to draw one crosshair through both.
    command_bar: Option<Rect>,
    editor_bar: Option<Rect>,
    /// The left outline panel, used to decide which text Ctrl+wheel resizes.
    outline_rect: Option<Rect>,
    crosshair_x: Option<f32>,
    /// Which dialog has been given the keyboard, so it is not grabbed again every frame.
    dialog_focus: Option<DialogKind>,
    /// The editor's Enter key was released while a dialog was open.
    dialog_confirm: bool,
    /// The window is closing, so open files go back onto the recent list.
    exiting: bool,
}

impl Window {
    fn new(cc: &eframe::CreationContext<'_>, session: Session) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(0, 0, 28);
        visuals.window_fill = Color32::from_rgb(0, 0, 36);
        visuals.override_text_color = Some(Color32::from_rgb(230, 230, 230));
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.options_mut(|options| {
            options.zoom_with_keyboard = false;
        });
        cc.egui_ctx.set_zoom_factor(1.0);
        let mut window = Self {
            view: EditorView::new(session),
            hscroll: 0,
            top_row: 0,
            page_rows: 24,
            visible_cols: 80,
            scroll_grab: None,
            close_prompt: false,
            allow_close: false,
            end_prompt: false,
            end_closes_window: false,
            reopen_prompt: false,
            help_open: false,
            options_open: false,
            options_draft: None,
            about_open: false,
            enter_key: EnterKey::BothCtrl,
            next_line_at: NextLineCursor::LineNumber,
            color_theme: ColorTheme::Pcom,
            left_ctrl: false,
            right_ctrl: false,
            attention_held: false,
            attention_used: false,
            jumps: Vec::new(),
            jump_at: 0,
            alt_held: false,
            pending_menu: None,
            active_top_menu: None,
            menu_select_pending: false,
            menu_descend: false,
            exit_after_save: false,
            swap_from: 0,
            swap_point: false,
            swap_list_at: None,
            outline_width: 220.0,
            outline_restore: 220.0,
            outline_drag: None,
            outline_follow: None,
            outline_hovered: false,
            recent_limit: 10,
            recent: Vec::new(),
            recent_open: false,
            tab_menu: None,
            tab_menu_fresh: false,
            editor_menu: None,
            editor_menu_fresh: false,
            tab_drag: None,
            show_toolbar: true,
            show_status_bar: true,
            zoom_outline: 0,
            zoom_editor: 0,
            zoom_command: 0,
            zoom_wheel: 0.0,
            zoom_wheel_part: None,
            crosshair: true,
            command_bar: None,
            editor_bar: None,
            outline_rect: None,
            crosshair_x: None,
            dialog_focus: None,
            dialog_confirm: false,
            exiting: false,
        };
        window.load_config();
        window.view.refresh_empty_status(window.enter_key.label());
        window.conceal_open_files();
        window
    }

    fn config_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.join("ispflite.config")))
    }

    fn load_config(&mut self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut recent = Vec::new();
        let mut saw_outline_restore = false;
        let mut legacy_zoom = None;
        let mut saw_zoom_outline = false;
        let mut saw_zoom_editor = false;
        let mut saw_zoom_command = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "enter" => {
                    self.enter_key = match value {
                        "enter" => EnterKey::Enter,
                        "right" => EnterKey::RightCtrl,
                        "left" => EnterKey::LeftCtrl,
                        _ => EnterKey::BothCtrl,
                    };
                }
                "next" => {
                    self.next_line_at = if value == "text" {
                        NextLineCursor::Text
                    } else {
                        NextLineCursor::LineNumber
                    };
                }
                "theme" => {
                    self.color_theme = if value == "dark" {
                        ColorTheme::Current
                    } else {
                        ColorTheme::Pcom
                    };
                }
                "recent_limit" => {
                    if let Ok(limit) = value.parse::<usize>() {
                        self.recent_limit = limit.clamp(1, 50);
                    }
                }
                "outline" => {
                    if let Ok(width) = value.parse::<f32>() {
                        self.outline_width = width.clamp(0.0, 480.0);
                    }
                }
                "outline_restore" => {
                    saw_outline_restore = true;
                    if let Ok(width) = value.parse::<f32>() {
                        self.outline_restore = width.clamp(1.0, 480.0);
                    }
                }
                "toolbar" => self.show_toolbar = value != "hide",
                "status_bar" => self.show_status_bar = value != "hide",
                "zoom" => {
                    if let Ok(step) = value.parse::<i32>() {
                        legacy_zoom = Some(step.clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP));
                    }
                }
                "zoom_outline" => {
                    saw_zoom_outline = true;
                    if let Ok(step) = value.parse::<i32>() {
                        self.zoom_outline = step.clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
                    }
                }
                "zoom_editor" => {
                    saw_zoom_editor = true;
                    if let Ok(step) = value.parse::<i32>() {
                        self.zoom_editor = step.clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
                    }
                }
                "zoom_command" => {
                    saw_zoom_command = true;
                    if let Ok(step) = value.parse::<i32>() {
                        self.zoom_command = step.clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
                    }
                }
                "recent" => {
                    if !value.is_empty() {
                        recent.push(std::path::PathBuf::from(value));
                    }
                }
                _ => {}
            }
        }
        recent.truncate(self.recent_limit);
        self.recent = recent;
        if !saw_outline_restore && self.outline_width > 1.0 {
            self.outline_restore = self.outline_width;
        }
        if let Some(step) = legacy_zoom {
            if !saw_zoom_outline {
                self.zoom_outline = step;
            }
            if !saw_zoom_editor {
                self.zoom_editor = step;
            }
            if !saw_zoom_command {
                self.zoom_command = step;
            }
        }
    }

    fn save_config(&self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        let enter = match self.enter_key {
            EnterKey::Enter => "enter",
            EnterKey::RightCtrl => "right",
            EnterKey::LeftCtrl => "left",
            EnterKey::BothCtrl => "both",
        };
        let next = match self.next_line_at {
            NextLineCursor::Text => "text",
            NextLineCursor::LineNumber => "number",
        };
        let theme = match self.color_theme {
            ColorTheme::Current => "dark",
            ColorTheme::Pcom => "classic",
        };
        let toolbar = if self.show_toolbar { "show" } else { "hide" };
        let status_bar = if self.show_status_bar { "show" } else { "hide" };
        let mut text = format!(
            "enter={enter}\nnext={next}\ntheme={theme}\nrecent_limit={}\noutline={}\noutline_restore={}\ntoolbar={toolbar}\nstatus_bar={status_bar}\nzoom_outline={}\nzoom_editor={}\nzoom_command={}\n",
            self.recent_limit,
            self.outline_width,
            self.outline_restore,
            self.zoom_outline,
            self.zoom_editor,
            self.zoom_command
        );
        for path in &self.recent {
            text.push_str("recent=");
            text.push_str(&path.display().to_string());
            text.push('\n');
        }
        let _ = std::fs::write(path, text);
    }

    fn handle_input(&mut self, ui: &egui::Ui) {
        if self.view.session.swap_list_open() {
            self.handle_swap_list_input(ui);
            return;
        }
        if self.dialog_open() {
            self.handle_dialog_input(ui);
            return;
        }
        let menu_open = egui::Popup::is_any_open(ui.ctx());
        if !menu_open {
            self.active_top_menu = None;
            self.menu_select_pending = false;
            self.menu_descend = false;
        }
        let events = ui.input(|input| input.events.clone());
        let held = ui.input(|input| input.modifiers);
        self.alt_held = held.alt;
        if menu_open && self.menu_select_pending {
            let arrows_held =
                ui.input(|input| input.key_down(Key::ArrowLeft) || input.key_down(Key::ArrowRight));
            if !held.alt && !arrows_held {
                self.menu_descend = true;
                self.menu_select_pending = false;
            }
        }
        for event in events {
            match event {
                Event::Copy if !menu_open => {
                    self.attention_used = true;
                    self.copy_selection(ui.ctx());
                }
                Event::Cut if !menu_open => {
                    self.attention_used = true;
                    self.cut_selection(ui.ctx());
                }
                Event::Paste(text) if !menu_open => {
                    self.attention_used = true;
                    self.view.paste_text(&text);
                }
                Event::Text(text) if !menu_open && !held.command && !held.ctrl && !held.alt => {
                    if self.attention_held {
                        self.attention_used = true;
                    }
                    for ch in text.chars() {
                        if !ch.is_control() {
                            self.view.handle_key(EditorKey::Char(ch));
                        }
                    }
                }
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if self.on_attention_ctrl(key, physical_key, pressed) {
                        continue;
                    }
                    self.note_ctrl(key, physical_key, pressed);
                    if pressed && self.attention_held {
                        self.attention_used = true;
                    }
                    if pressed
                        && modifiers.alt
                        && !modifiers.ctrl
                        && !modifiers.shift
                        && !modifiers.command
                    {
                        if let Some(menu) = menu_mnemonic(key) {
                            self.pending_menu = Some(menu);
                            self.active_top_menu = Some(menu);
                            continue;
                        }
                    }
                    if menu_open {
                        // Leave Up/Down, Enter, and the rest to egui's menu focus.
                        // Left/Right step File ↔ Edit ↔ View ↔ Window ↔ Help.
                        if pressed
                            && !modifiers.alt
                            && !modifiers.ctrl
                            && !modifiers.shift
                            && !modifiers.command
                            && matches!(key, Key::ArrowLeft | Key::ArrowRight)
                        {
                            if let Some(next) =
                                neighbor_top_menu(self.active_top_menu, key == Key::ArrowRight)
                            {
                                self.pending_menu = Some(next);
                                self.active_top_menu = Some(next);
                                // First item is selected when the arrow is released.
                                self.menu_select_pending = true;
                                self.menu_descend = false;
                                // Stop egui from treating Left as "back to the menu button".
                                ui.ctx().memory_mut(|mem| {
                                    mem.move_focus(egui::FocusDirection::None);
                                });
                            }
                        }
                        continue;
                    }
                    if pressed {
                        self.handle_key(ui.ctx(), key, physical_key, modifiers);
                    }
                }
                Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    source,
                    ..
                } => {
                    let ctrl = modifiers.ctrl || modifiers.command;
                    if ctrl && !modifiers.alt && !matches!(source, egui::MouseWheelSource::Momentum)
                    {
                        self.note_zoom_wheel(ui, unit, delta.y);
                    } else {
                        self.zoom_wheel = 0.0;
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_swap_list_input(&mut self, ui: &egui::Ui) {
        let events = ui.input(|input| input.events.clone());
        let last = self.view.session.tabs().len();
        if self.swap_list_at.is_none() {
            self.swap_list_at = Some(self.view.session.current_index().min(last));
        }
        for event in events {
            let Event::Key {
                key, pressed: true, ..
            } = event
            else {
                continue;
            };
            let at = self.swap_list_at.unwrap_or(0).min(last);
            match key {
                Key::ArrowUp => self.swap_list_at = Some(move_swap_list(at, last, false)),
                Key::ArrowDown => self.swap_list_at = Some(move_swap_list(at, last, true)),
                Key::Enter => self.choose_swap_list(at),
                Key::Escape => {
                    self.view.session.set_swap_list(false);
                    self.swap_list_at = None;
                }
                _ => {}
            }
        }
    }

    fn handle_dialog_input(&mut self, ui: &egui::Ui) {
        let events = ui.input(|input| input.events.clone());
        let popup = egui::Popup::is_any_open(ui.ctx());
        for event in events {
            let Event::Key {
                key,
                physical_key,
                pressed,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            if self.attention_event(key, physical_key, pressed, false) {
                continue;
            }
            self.note_ctrl(key, physical_key, pressed);
            if pressed && self.attention_held {
                self.attention_used = true;
            }
            if pressed && key == Key::Escape && !modifiers.ctrl && !modifiers.alt && !popup {
                self.dismiss_dialog();
            }
        }
    }

    fn choose_swap_list(&mut self, at: usize) {
        let screens = self.view.session.tabs().len();
        if at >= screens {
            self.start_new_screen();
        } else {
            let _ = self.view.session.activate_screen(at);
        }
        self.view.session.set_swap_list(false);
        self.swap_list_at = None;
    }

    /// Ctrl assigned as Enter runs on key release, and only when it was not
    /// held as a modifier for another key. Returns true when the event is consumed.
    fn on_attention_ctrl(&mut self, key: Key, physical: Option<Key>, pressed: bool) -> bool {
        self.attention_event(key, physical, pressed, true)
    }

    /// Tracks the key that means Enter. `editor` sends that press to the file;
    /// otherwise it confirms the focused control in the open dialog.
    fn attention_event(
        &mut self,
        key: Key,
        physical: Option<Key>,
        pressed: bool,
        editor: bool,
    ) -> bool {
        if !is_attention_ctrl(self.enter_key, key, physical) {
            return false;
        }
        if pressed {
            if !self.attention_held {
                self.attention_used = false;
            }
            self.attention_held = true;
            self.note_ctrl(key, physical, true);
            return true;
        }
        self.note_ctrl(key, physical, false);
        let still = self.attention_ctrl_down();
        let fire = release_runs_enter(self.attention_held, self.attention_used, still);
        if !still {
            self.attention_held = false;
            self.attention_used = false;
        }
        if fire {
            if editor {
                self.view.handle_key(EditorKey::Enter);
            } else {
                self.dialog_confirm = true;
            }
        }
        true
    }

    fn attention_ctrl_down(&self) -> bool {
        match self.enter_key {
            EnterKey::Enter => false,
            EnterKey::RightCtrl => self.right_ctrl,
            EnterKey::LeftCtrl => self.left_ctrl,
            EnterKey::BothCtrl => self.left_ctrl || self.right_ctrl,
        }
    }

    fn copy_selection(&mut self, ctx: &egui::Context) {
        if let Some(text) = self.view.copy_selection() {
            ctx.copy_text(text);
        }
    }

    fn cut_selection(&mut self, ctx: &egui::Context) {
        if let Some(text) = self.view.cut_selection() {
            ctx.copy_text(text);
        }
    }

    fn paste_from_clipboard(&mut self) {
        #[cfg(windows)]
        if let Ok(text) = clipboard_win::get_clipboard_string() {
            self.view.paste_text(&text);
        }
    }

    fn note_ctrl(&mut self, key: Key, physical: Option<Key>, pressed: bool) {
        match physical.unwrap_or(key) {
            Key::ControlLeft => self.left_ctrl = pressed,
            Key::ControlRight => self.right_ctrl = pressed,
            _ => {}
        }
    }

    fn handle_key(
        &mut self,
        ctx: &egui::Context,
        key: Key,
        physical: Option<Key>,
        modifiers: Modifiers,
    ) {
        if key == Key::Escape && self.dismiss_dialog() {
            return;
        }
        if key == Key::Escape && (self.tab_menu.is_some() || self.editor_menu.is_some()) {
            self.tab_menu = None;
            self.editor_menu = None;
            return;
        }
        if self.swap_point && self.view.session.swapbar() && matches!(key, Key::F10 | Key::F11) {
            let last = self.view.session.tabs().len().saturating_sub(1);
            if key == Key::F10 {
                self.swap_from = self.swap_from.saturating_sub(1);
            } else {
                self.swap_from = (self.swap_from + 1).min(last);
            }
            return;
        }
        self.swap_point = false;
        self.view.note_screen(self.top_row, self.page_rows.max(1));
        if key == Key::Home && modifiers.ctrl && !modifiers.shift && !modifiers.alt {
            self.crosshair = !self.crosshair;
            return;
        }
        let command = ctrl_modifies(
            self.enter_key,
            self.left_ctrl,
            self.right_ctrl,
            modifiers.ctrl || modifiers.command,
        );
        let shortcut = (modifiers.ctrl || modifiers.command) && !modifiers.alt;
        if shortcut && key == Key::N && !modifiers.shift {
            self.start_new_screen();
            return;
        }
        if shortcut && key == Key::O && !modifiers.shift {
            self.open_dialog();
            return;
        }
        if shortcut && key == Key::O && modifiers.shift {
            self.options_open = true;
            return;
        }
        if shortcut && key == Key::S && !modifiers.shift {
            self.save_current();
            return;
        }
        if shortcut && key == Key::S && modifiers.shift {
            self.save_as_dialog();
            return;
        }
        if shortcut && key == Key::R && !modifiers.shift {
            self.reopen_current();
            return;
        }
        if shortcut && key == Key::R && modifiers.shift {
            self.recent_open = true;
            return;
        }
        if shortcut && key == Key::W && !modifiers.shift {
            self.end_file(ctx);
            return;
        }
        if shortcut && key == Key::A && !modifiers.shift {
            self.view.select_all();
            return;
        }
        if shortcut && (key == Key::Plus || (key == Key::Equals && modifiers.shift)) {
            self.zoom_in();
            return;
        }
        if shortcut && key == Key::Minus && !modifiers.shift {
            self.zoom_out();
            return;
        }
        if shortcut && key == Key::Num0 && !modifiers.shift {
            self.zoom_reset();
            return;
        }
        if shortcut && key == Key::Z && !modifiers.shift {
            self.view.handle_key(EditorKey::Undo);
            return;
        }
        if shortcut && (key == Key::Y || (key == Key::Z && modifiers.shift)) {
            self.view.handle_key(EditorKey::Redo);
            return;
        }
        if command && key == Key::Home {
            self.view.handle_key(EditorKey::Top);
            return;
        }
        if command && key == Key::End {
            self.view.handle_key(EditorKey::Bottom);
            return;
        }
        match key {
            Key::F1 => {
                self.help_open = true;
                return;
            }
            Key::F2 => {
                self.view.set_status("Split is not available");
                return;
            }
            Key::F3 => {
                self.end_file(ctx);
                return;
            }
            Key::F4 => {
                self.view.run_primary("SWAP LIST");
                return;
            }
            Key::F5 => {
                if modifiers.shift {
                    self.view.run_primary("RFIND PREV");
                } else {
                    self.view.run_primary("RFIND");
                }
                return;
            }
            Key::F6 => {
                self.view.run_primary("RCHANGE");
                return;
            }
            Key::F9 => {
                self.view.swap_key();
                return;
            }
            Key::F12 => {
                self.view.retrieve(modifiers.shift);
                return;
            }
            _ => {}
        }
        if let Some(dir) = match key {
            Key::F7 | Key::PageUp => Some(ScrollDir::Up),
            Key::F8 | Key::PageDown => Some(ScrollDir::Down),
            Key::F10 => Some(ScrollDir::Left),
            Key::F11 => Some(ScrollDir::Right),
            _ => None,
        } {
            if matches!(dir, ScrollDir::Up | ScrollDir::Down) && self.view.take_max_scroll() {
                self.view.scroll_max(dir == ScrollDir::Up);
                return;
            }
            if matches!(dir, ScrollDir::Up | ScrollDir::Down) {
                if let Some(lines) = self.view.take_line_scroll() {
                    let new_top = self.view.scroll_by_lines(
                        dir == ScrollDir::Down,
                        lines,
                        self.top_row,
                        self.page_rows,
                    );
                    self.scroll_keeping_line(new_top);
                    return;
                }
            }
            if matches!(dir, ScrollDir::Left | ScrollDir::Right) && self.view.take_max_scroll() {
                self.hscroll = self
                    .view
                    .scroll_max_columns(dir == ScrollDir::Right, self.visible_cols.max(1));
                return;
            }
            if matches!(dir, ScrollDir::Left | ScrollDir::Right) {
                if let Some(cols) = self.view.take_line_scroll() {
                    self.hscroll =
                        self.view
                            .scroll_by_columns(dir == ScrollDir::Right, cols, self.hscroll);
                    if dir == ScrollDir::Left {
                        self.view
                            .land_on_rightmost_if_outside(self.hscroll, self.visible_cols.max(1));
                    }
                    return;
                }
            }
            let (top, left) = self.view.scroll_by_cursor(
                dir,
                self.page_rows.max(1),
                self.visible_cols.max(1),
                self.top_row,
                self.hscroll,
            );
            self.hscroll = left;
            if dir == ScrollDir::Left {
                self.view
                    .land_on_rightmost_if_outside(self.hscroll, self.visible_cols.max(1));
            }
            if matches!(dir, ScrollDir::Up | ScrollDir::Down) {
                self.scroll_keeping_line(top);
            }
            return;
        }
        if matches!(
            key,
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown
        ) {
            let editor_key = match key {
                Key::ArrowLeft => EditorKey::Left,
                Key::ArrowRight => EditorKey::Right,
                Key::ArrowUp => EditorKey::Up,
                Key::ArrowDown => EditorKey::Down,
                _ => return,
            };
            if modifiers.ctrl && key == Key::ArrowLeft {
                self.jump_back();
                return;
            }
            if modifiers.ctrl && key == Key::ArrowRight {
                self.jump_forward();
                return;
            }
            if modifiers.alt && matches!(key, Key::ArrowLeft | Key::ArrowRight) {
                self.view
                    .jump_nonspace(key == Key::ArrowRight, modifiers.shift);
                return;
            }
            self.view.nudge(editor_key, modifiers.shift);
            return;
        }
        if is_attention(self.enter_key, key, physical) {
            self.view.handle_key(EditorKey::Enter);
            return;
        }
        if key == Key::Enter && self.enter_key != EnterKey::Enter {
            self.view.next_line(self.next_line_at, self.top_row);
            return;
        }
        let editor_key = match key {
            Key::Backspace => EditorKey::Backspace,
            Key::Delete => EditorKey::Delete,
            Key::Home => EditorKey::Home,
            Key::End => EditorKey::End,
            Key::Tab if modifiers.shift => EditorKey::BackTab,
            Key::Tab => EditorKey::Tab,
            Key::Insert => EditorKey::ToggleInsert,
            Key::Escape => {
                self.view.handle_key(EditorKey::Escape);
                return;
            }
            _ => return,
        };
        let _ = ctx;
        self.view.handle_key(editor_key);
    }

    fn open_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("COBOL (*.cbl)", &["cbl"])
            .add_filter("All files (*.*)", &["*"]);
        if let Some(path) = self.view.session.current().path.as_ref() {
            if let Some(dir) = path.parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        if let Some(paths) = dialog.pick_files() {
            self.open_paths(paths);
        }
    }

    fn open_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        let mut switched = false;
        for path in paths {
            if let Some(index) = self.view.session.index_of_path(&path) {
                if index != self.view.session.current_index() {
                    let _ = self.view.session.select(index);
                    switched = true;
                }
                self.view.set_status("Already open");
                continue;
            }
            self.view.open_path(&path);
            if self
                .view
                .session
                .current()
                .path
                .as_ref()
                .is_some_and(|open| same_file(open, &path))
            {
                switched = true;
            }
        }
        if switched {
            self.hscroll = 0;
            self.top_row = 0;
            let (line, col) = self.view.cursor();
            self.view.jump_to_line(line, col);
        }
    }

    fn remember_recent(&mut self, path: std::path::PathBuf) {
        push_recent(&mut self.recent, path, self.recent_limit);
    }

    /// An open file stays off the recent list.
    fn conceal_open_files(&mut self) {
        let open: Vec<std::path::PathBuf> = self
            .view
            .session
            .tabs()
            .iter()
            .filter_map(|tab| tab.path.clone())
            .collect();
        let before = self.recent.clone();
        drop_open_paths(&mut self.recent, &open);
        if self.recent != before {
            self.save_config();
        }
    }

    /// Put a path back when no screen still has it.
    fn release_if_unused(&mut self, path: Option<std::path::PathBuf>) {
        let Some(path) = path else {
            return;
        };
        if self.view.session.index_of_path(&path).is_some() {
            return;
        }
        self.remember_recent(path);
        self.save_config();
    }

    fn return_open_files_to_recent(&mut self) {
        let current_id = self.view.session.current().id;
        let mut others = Vec::new();
        let mut current = None;
        for tab in self.view.session.tabs() {
            let Some(path) = tab.path.clone() else {
                continue;
            };
            if tab.id == current_id {
                current = Some(path);
            } else {
                others.push(path);
            }
        }
        for path in others {
            self.remember_recent(path);
        }
        if let Some(path) = current {
            self.remember_recent(path);
        }
    }

    fn save_current(&mut self) {
        if self.view.session.current().path.is_none() {
            self.save_as_dialog();
        } else {
            self.view.save();
        }
    }

    fn select_tab(&mut self, id: u32) -> bool {
        let Some(index) = self.view.session.tabs().iter().position(|tab| tab.id == id) else {
            return false;
        };
        if self.view.session.current().id != id {
            let _ = self.view.session.select(index);
            self.hscroll = 0;
            self.top_row = 0;
            let line = self.view.cursor().0;
            self.view.click(PointerTarget::Data { line, col: 1 });
        }
        true
    }

    fn move_tab_to_new_instance(&mut self, ctx: &egui::Context) {
        if self.view.session.tabs().len() < 2 {
            return;
        }
        if self.view.session.current().path.is_none() {
            self.view.set_status("Save the file before moving it");
            return;
        }
        if self.view.session.current().doc.is_modified() && !self.view.save() {
            return;
        }
        let Some(path) = self.view.session.current().path.clone() else {
            self.view.set_status("Save the file before moving it");
            return;
        };
        if !spawn_instance(&path, &mut self.view) {
            return;
        }
        let last = self.view.session.tabs().len() == 1;
        self.finish_end(ctx, last);
        if !last {
            self.view.set_status("Opened in a new instance");
        }
    }

    fn save_as_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save As")
            .add_filter("COBOL (*.cbl)", &["cbl"])
            .add_filter("All files (*.*)", &["*"]);
        if let Some(path) = self.view.session.current().path.as_ref() {
            if let Some(dir) = path.parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        if let Some(path) = dialog.save_file() {
            let previous = self.view.session.current().path.clone();
            if self.view.save_as(&path) {
                self.release_if_unused(previous);
            } else {
                self.exit_after_save = false;
            }
        } else {
            self.exit_after_save = false;
        }
    }

    /// Move the window and leave the cursor on its line.
    fn scroll_window(&mut self, requested: usize) {
        let count = self.view.session.current().doc.visible_rows().len();
        let page = self.page_rows.max(1);
        let max_top = count.saturating_sub(page);
        self.top_row = requested.min(max_top);
    }

    /// Move the viewport and keep the cursor on the same screen row.
    fn scroll_vertical(&mut self, requested: usize) {
        let count = self.view.session.current().doc.visible_rows().len();
        let page = self.page_rows.max(1);
        let max_top = count.saturating_sub(page);
        let new_top = requested.min(max_top);
        let old = self.top_row.min(max_top);
        self.top_row = new_top;
        if new_top != old {
            self.view.slide_with_scroll(old, new_top, page);
        }
    }

    /// F7 and F8 move the window, then leave the cursor on its line when that
    /// line is still on the screen.
    fn scroll_keeping_line(&mut self, requested: usize) {
        let count = self.view.session.current().doc.visible_rows().len();
        let page = self.page_rows.max(1);
        let full_page = count.saturating_sub(page);
        let last = count.saturating_sub(1);
        let new_top = if requested > full_page {
            requested.min(last)
        } else {
            requested
        };
        self.top_row = new_top;
        if self.view.focus() != Focus::Command {
            self.view.keep_line_if_visible(new_top, page);
        }
    }

    fn handle_scrollbar(&mut self, ui: &egui::Ui, bar: Rect, row_count: usize) {
        let page = self.page_rows.max(1);
        let max_top = row_count.saturating_sub(page);
        let response = ui.interact(
            bar,
            ui.scope_id().with("editor_vscroll"),
            Sense::CLICK | Sense::DRAG,
        );
        let mut thumb = scrollbar_thumb(bar, self.top_row, max_top, page, row_count);
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                self.scroll_grab = Some(if thumb.contains(pos) {
                    pos.y - thumb.top()
                } else {
                    thumb.height() * 0.5
                });
            }
        }
        if response.dragged() {
            if let (Some(pos), Some(grab)) = (response.interact_pointer_pos(), self.scroll_grab) {
                let travel = (bar.height() - thumb.height()).max(1.0);
                let y = (pos.y - bar.top() - grab).clamp(0.0, travel);
                let new_top = if max_top == 0 {
                    0
                } else {
                    ((y / travel) * max_top as f32).round() as usize
                };
                self.scroll_vertical(new_top);
                thumb = scrollbar_thumb(bar, self.top_row, max_top, page, row_count);
            }
        } else {
            self.scroll_grab = None;
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if pos.y < thumb.top() {
                    self.scroll_vertical(self.top_row.saturating_sub(page));
                } else if pos.y > thumb.bottom() {
                    self.scroll_vertical(self.top_row.saturating_add(page));
                }
                thumb = scrollbar_thumb(bar, self.top_row, max_top, page, row_count);
            }
        }
        ui.painter()
            .rect_filled(bar, 0.0, Color32::from_rgb(6, 10, 28));
        let thumb_color = if response.dragged() || response.hovered() {
            Color32::from_rgb(120, 160, 220)
        } else {
            Color32::from_rgb(70, 110, 170)
        };
        ui.painter().rect_filled(thumb, 2.0, thumb_color);
    }

    fn text_px(&self, step: i32) -> f32 {
        (FONT_SIZE * 1.1_f32.powi(step)).clamp(8.0, 48.0)
    }

    fn outline_px(&self) -> f32 {
        self.text_px(self.zoom_outline)
    }

    fn editor_px(&self) -> f32 {
        self.text_px(self.zoom_editor)
    }

    fn command_px(&self) -> f32 {
        self.text_px(self.zoom_command)
    }

    fn zoom_step(&self, part: TextZoom) -> i32 {
        match part {
            TextZoom::Outline => self.zoom_outline,
            TextZoom::Editor => self.zoom_editor,
            TextZoom::Command => self.zoom_command,
        }
    }

    fn store_zoom_step(&mut self, part: TextZoom, step: i32) {
        match part {
            TextZoom::Outline => self.zoom_outline = step,
            TextZoom::Editor => self.zoom_editor = step,
            TextZoom::Command => self.zoom_command = step,
        }
    }

    fn set_zoom_step(&mut self, part: TextZoom, step: i32) {
        let step = step.clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
        if self.zoom_step(part) == step {
            return;
        }
        self.store_zoom_step(part, step);
        self.save_config();
    }

    fn zoom_all(&mut self, step: impl Fn(i32) -> i32) {
        let outline = step(self.zoom_outline).clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
        let editor = step(self.zoom_editor).clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
        let command = step(self.zoom_command).clamp(ZOOM_MIN_STEP, ZOOM_MAX_STEP);
        if outline == self.zoom_outline
            && editor == self.zoom_editor
            && command == self.zoom_command
        {
            return;
        }
        self.zoom_outline = outline;
        self.zoom_editor = editor;
        self.zoom_command = command;
        self.save_config();
    }

    fn zoom_in(&mut self) {
        self.zoom_all(|step| step + 1);
    }

    fn zoom_out(&mut self) {
        self.zoom_all(|step| step - 1);
    }

    fn zoom_reset(&mut self) {
        self.zoom_all(|_| 0);
    }

    fn zoom_target(&self, ui: &egui::Ui) -> Option<TextZoom> {
        let pos = ui.input(|input| input.pointer.hover_pos())?;
        if self.command_bar.is_some_and(|rect| rect.contains(pos)) {
            Some(TextZoom::Command)
        } else if self.outline_rect.is_some_and(|rect| rect.contains(pos)) {
            Some(TextZoom::Outline)
        } else if self.editor_bar.is_some_and(|rect| rect.contains(pos)) {
            Some(TextZoom::Editor)
        } else {
            None
        }
    }

    fn note_zoom_wheel(&mut self, ui: &egui::Ui, unit: egui::MouseWheelUnit, delta_y: f32) {
        let Some(part) = self.zoom_target(ui) else {
            self.zoom_wheel = 0.0;
            self.zoom_wheel_part = None;
            return;
        };
        if self.zoom_wheel_part != Some(part) {
            self.zoom_wheel = 0.0;
            self.zoom_wheel_part = Some(part);
        }
        let lines = match unit {
            egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta_y,
            egui::MouseWheelUnit::Point => delta_y / 120.0,
        };
        self.zoom_wheel += lines;
        while self.zoom_wheel >= 1.0 {
            self.zoom_wheel -= 1.0;
            self.set_zoom_step(part, self.zoom_step(part) + 1);
        }
        while self.zoom_wheel <= -1.0 {
            self.zoom_wheel += 1.0;
            self.set_zoom_step(part, self.zoom_step(part) - 1);
        }
    }

    fn hide_outline(&mut self) {
        if self.outline_width > 1.0 {
            self.outline_restore = self.outline_width;
        }
        self.outline_width = 0.0;
    }

    fn toggle_outline(&mut self) {
        if self.outline_width > 0.0 {
            self.hide_outline();
        } else {
            let width = if self.outline_restore > 1.0 {
                self.outline_restore
            } else {
                220.0
            };
            self.outline_width = width.clamp(1.0, 480.0);
            self.outline_restore = self.outline_width;
        }
        self.save_config();
    }

    fn paint_outline(&mut self, ui: &mut egui::Ui, height: f32) {
        self.outline_hovered = false;
        let entries = {
            let tab = self.view.session.current();
            let found = cobol_outline(&tab.doc);
            if found.is_empty() && !cobol_path(tab.path.as_deref()) {
                return;
            }
            found
        };
        let cursor_line = self.view.cursor().0;
        let current = entries
            .iter()
            .rev()
            .find(|entry| entry.line <= cursor_line)
            .map(|entry| entry.line);
        let tab_id = self.view.session.current().id;
        let follow = current.map(|line| (tab_id, line));
        let reveal = follow != self.outline_follow;
        let font = FontId::monospace(self.outline_px());
        let char_w = ui
            .painter()
            .fonts_mut(|fonts| fonts.glyph_width(&font, 'M').max(1.0));
        let min_show = char_w * 3.0;
        let width = self.outline_width.clamp(0.0, 480.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if width > 0.0 {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(width);
                        ui.set_max_width(width);
                        let panel = Rect::from_min_size(ui.cursor().min, Vec2::new(width, height));
                        self.outline_rect = Some(panel);
                        self.outline_hovered = ui.rect_contains_pointer(panel);
                        ui.set_clip_rect(panel.intersect(ui.clip_rect()));
                        ui.painter().rect_filled(panel, 0.0, Color32::BLACK);
                        ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                        ui.visuals_mut().selection.bg_fill = Color32::from_rgb(40, 40, 40);
                        ui.visuals_mut().selection.stroke.color = Color32::WHITE;
                        ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::BLACK;
                        ui.visuals_mut().widgets.hovered.weak_bg_fill =
                            Color32::from_rgb(24, 24, 24);
                        ui.visuals_mut().widgets.active.weak_bg_fill =
                            Color32::from_rgb(40, 40, 40);
                        let outline_px = self.outline_px();
                        ui.label(
                            egui::RichText::new("COBOL")
                                .monospace()
                                .size(outline_px)
                                .color(Color32::WHITE),
                        );
                        let cross_side = 16.0;
                        let cross_rect = Rect::from_min_size(
                            Pos2::new(panel.right() - cross_side - 4.0, panel.top() + 3.0),
                            Vec2::splat(cross_side),
                        );
                        let cross = ui.interact(
                            cross_rect,
                            ui.scope_id().with("outline_hide"),
                            Sense::click(),
                        );
                        if cross.hovered() {
                            ui.painter().rect_filled(
                                cross_rect,
                                2.0,
                                Color32::from_rgb(40, 40, 40),
                            );
                        }
                        let stroke = Stroke::new(1.7, Color32::WHITE);
                        let inset = 3.5;
                        ui.painter().line_segment(
                            [
                                Pos2::new(cross_rect.left() + inset, cross_rect.top() + inset),
                                Pos2::new(cross_rect.right() - inset, cross_rect.bottom() - inset),
                            ],
                            stroke,
                        );
                        ui.painter().line_segment(
                            [
                                Pos2::new(cross_rect.right() - inset, cross_rect.top() + inset),
                                Pos2::new(cross_rect.left() + inset, cross_rect.bottom() - inset),
                            ],
                            stroke,
                        );
                        if cross.clicked() {
                            self.hide_outline();
                            self.outline_drag = None;
                            self.save_config();
                        }
                        ui.spacing_mut().scroll.fade.strength = 0.0;
                        let header_h = (ui.cursor().min.y - panel.top())
                            .max(cross_rect.bottom() - panel.top() + 2.0);
                        egui::ScrollArea::vertical()
                            .id_salt("cobol_outline")
                            .auto_shrink([false, false])
                            .max_height((height - header_h).max(0.0))
                            .show(ui, |ui| {
                                for entry in &entries {
                                    let selected = current == Some(entry.line);
                                    let label = entry.label();
                                    let text_color = if selected {
                                        Color32::BLACK
                                    } else {
                                        Color32::WHITE
                                    };
                                    let galley = ui.painter().layout_no_wrap(
                                        label.clone(),
                                        FontId::monospace(outline_px),
                                        text_color,
                                    );
                                    let row_w = ui.available_width();
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(row_w, galley.size().y),
                                        Sense::CLICK,
                                    );
                                    if selected {
                                        ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
                                    }
                                    ui.painter().galley(rect.min, galley, text_color);
                                    if selected && reveal {
                                        response.scroll_to_me(None);
                                    }
                                    if response.clicked() {
                                        self.swap_point = false;
                                        self.view.jump_to_line(entry.line, 1);
                                    }
                                }
                                self.outline_follow = follow;
                            });
                    },
                );
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::new(6.0, height), Sense::hover());
            let response = ui
                .interact(
                    rect,
                    ui.scope_id().with("outline_divider"),
                    Sense::CLICK | Sense::DRAG,
                )
                .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
            ui.painter().rect_filled(
                rect,
                0.0,
                if response.hovered() || response.dragged() {
                    Color32::from_rgb(90, 140, 190)
                } else {
                    Color32::from_rgb(40, 60, 90)
                },
            );
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.outline_drag = Some((pos.x, self.outline_width));
                }
            }
            if let Some((origin_x, origin_width)) = self.outline_drag {
                if let Some(pos) = response.interact_pointer_pos() {
                    let next = origin_width + (pos.x - origin_x);
                    self.outline_width = if next < min_show {
                        0.0
                    } else {
                        next.min(480.0)
                    };
                }
            }
            if response.double_clicked() {
                self.outline_drag = None;
                self.toggle_outline();
            } else if response.drag_stopped() {
                let origin = self.outline_drag.map(|(_, width)| width);
                self.outline_drag = None;
                if self.outline_width < 1.0 {
                    if let Some(width) = origin.filter(|width| *width > 1.0) {
                        self.outline_restore = width;
                    }
                    self.outline_width = 0.0;
                } else {
                    self.outline_restore = self.outline_width;
                }
                self.save_config();
            }
        });
    }

    fn paint_editor(&mut self, ui: &mut egui::Ui) {
        let font = FontId::monospace(self.editor_px());
        let (char_w, row_h) = ui.painter().fonts_mut(|fonts| {
            (
                fonts.glyph_width(&font, 'M').max(1.0),
                fonts.row_height(&font).max(self.editor_px()),
            )
        });
        let prefix_w = char_w * PREFIX_COLS as f32;
        let gap = char_w;
        let bar_w = 14.0;
        let width = ui.available_width();
        let height = ui.available_height();
        let data_width = (width - bar_w).max(char_w);
        let visible_cols = ((data_width - prefix_w - gap) / char_w).floor().max(1.0) as usize;
        self.visible_cols = visible_cols;
        let cols_on = self.view.session.current().doc.cols_mode();
        let cols_rows = if cols_on { 1 } else { 0 };
        // The status line is already outside this area. Only the COLS ruler
        // takes a row away from the text.
        self.page_rows = ((height / row_h).floor() as usize)
            .saturating_sub(cols_rows)
            .max(1);
        ui.spacing_mut().item_spacing = Vec2::ZERO;

        let rows = self.view.session.current().doc.visible_rows();
        let page = self.page_rows.max(1);
        let last_top = rows.len().saturating_sub(1);
        if self.top_row > last_top {
            self.top_row = last_top;
        }
        let wheel = ui.input(|input| input.smooth_scroll_delta.y);
        if !self.outline_hovered && wheel > 8.0 {
            self.scroll_window(self.top_row.saturating_sub(3));
        } else if !self.outline_hovered && wheel < -8.0 {
            self.scroll_window(self.top_row.saturating_add(3));
        }
        let editor_origin = ui.cursor().min;
        self.editor_bar = Some(Rect::from_min_size(editor_origin, Vec2::new(width, height)));
        ui.shrink_clip_rect(Rect::from_min_size(editor_origin, Vec2::new(width, height)));
        let bar_rect = Rect::from_min_size(
            Pos2::new(editor_origin.x + data_width, editor_origin.y),
            Vec2::new(bar_w, height),
        );
        self.handle_scrollbar(ui, bar_rect, rows.len());
        let follow = self.view.take_follow();
        let second = self.view.take_scroll_second();
        if follow || second {
            self.reveal_column(visible_cols);
            let cursor_line = self.view.cursor().0;
            let idx = rows
                .iter()
                .position(|row| row.line == cursor_line)
                .unwrap_or(0);
            self.top_row = if second {
                if page > 1 {
                    top_for_second_line(idx)
                } else {
                    idx
                }
            } else {
                revealed_top(self.top_row, idx, page)
            };
        }
        let cursor = self.view.cursor();
        let focus = self.view.focus();
        let hscroll = self.hscroll;
        let prefixes: Vec<(usize, String, bool)> = rows
            .iter()
            .map(|row| {
                (
                    row.line,
                    self.view.shown_prefix(row.line, &row.prefix),
                    self.view.prefix_pending(row.line),
                )
            })
            .collect();

        let doc_flags: Vec<(bool, bool, bool)> = rows
            .iter()
            .map(|row| {
                let line = self.view.session.current().doc.line(row.line).ok();
                (
                    line.is_some_and(|item| item.is_top() || item.is_bottom()),
                    line.is_some_and(|item| item.is_xclude()),
                    line.is_some_and(|item| item.is_data()),
                )
            })
            .collect();

        let end = (self.top_row + page).min(rows.len());
        let path = self.view.session.current().path.clone();
        let mut carry = ColorCarry::default();
        let mut colored = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            if index >= end {
                break;
            }
            let (sentinel, exclude, _) = doc_flags[index];
            if sentinel || exclude {
                if index >= self.top_row {
                    colored.push(Vec::new());
                }
                continue;
            }
            let shown = self.view.shown_line(row.line, &row.text);
            let (kinds, next) = color_line(&shown, path.as_deref(), carry);
            carry = next;
            if index >= self.top_row {
                colored.push(kinds.into_iter().skip(hscroll).collect());
            }
        }

        let colors = syntax_colors(self.color_theme);
        ui.painter().rect_filled(
            Rect::from_min_size(editor_origin, Vec2::new(data_width, height)),
            0.0,
            colors.background,
        );

        if cols_on {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(data_width, row_h), Sense::hover());
            ui.painter().rect_filled(rect, 0.0, colors.cols_background);
            paint_string(ui, rect.min, "=COLS>  ", &font, colors.cols, char_w);
            let scale = column_scale(self.hscroll + 1, visible_cols);
            paint_string(
                ui,
                Pos2::new(rect.min.x + prefix_w + gap, rect.min.y),
                &scale,
                &font,
                colors.cols,
                char_w,
            );
        }

        let link_ctrl = ui.input(|input| input.modifiers.ctrl);
        let names = if link_ctrl {
            let tab = self.view.session.current();
            let folder = tab.path.as_ref().and_then(|path| path.parent());
            Some(name_index(&tab.doc, folder))
        } else {
            None
        };

        for (slot, index) in (self.top_row..end).enumerate() {
            let row = &rows[index];
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(data_width, row_h), Sense::CLICK);
            let current = row.line == cursor.0;
            let bg = if current {
                colors.current_line
            } else {
                colors.background
            };
            ui.painter().rect_filled(rect, 0.0, bg);
            let (sentinel, exclude, _) = doc_flags[index];
            let prefix_color = if exclude {
                Color32::from_rgb(255, 186, 92)
            } else if prefixes[index].2 {
                Color32::from_rgb(255, 230, 120)
            } else {
                LINE_NUMBER_COLOR
            };
            let text_color = if sentinel {
                colors.sentinel
            } else {
                colors.text
            };
            paint_string(
                ui,
                rect.min,
                &prefixes[index].1,
                &font,
                prefix_color,
                char_w,
            );
            let banner = if sentinel {
                Some(sentinel_line(&row.text))
            } else if exclude {
                Some(exclude_summary(row.exclude_count.unwrap_or(0)))
            } else {
                None
            };
            let shown = self.view.shown_line(row.line, &row.text);
            let source = banner.as_deref().unwrap_or(shown.as_ref());
            let chars: Vec<char> = source.chars().skip(hscroll).collect();
            let body_origin = Pos2::new(rect.min.x + prefix_w + gap, rect.min.y);
            let kinds = colored.get(slot).map(Vec::as_slice).unwrap_or(&[]);
            paint_chars(
                ui,
                body_origin,
                &chars,
                kinds,
                &font,
                text_color,
                &colors,
                char_w,
                visible_cols,
            );
            if let Some((start, end)) = self.view.data_selection(row.line) {
                paint_find_marks(
                    ui,
                    body_origin,
                    &[(start, end.saturating_sub(start))],
                    &chars,
                    &font,
                    hscroll,
                    char_w,
                    row_h,
                    visible_cols,
                );
            }
            paint_find_marks(
                ui,
                body_origin,
                &self.view.shown_find_spans(row.line),
                &chars,
                &font,
                hscroll,
                char_w,
                row_h,
                visible_cols,
            );
            let mut links = Vec::new();
            if let Some(index) = names.as_ref() {
                links.extend(paragraph_calls(shown.as_ref(), &index.paragraphs));
                for item in variable_uses(shown.as_ref(), row.line, &index.variables) {
                    let overlaps = links.iter().any(|link| {
                        link.col < item.col + item.len && item.col < link.col + link.len
                    });
                    if !overlaps {
                        links.push(item);
                    }
                }
                for item in copy_name_links(shown.as_ref(), &index.copies) {
                    let overlaps = links.iter().any(|link| {
                        link.col < item.col + item.len && item.col < link.col + link.len
                    });
                    if !overlaps {
                        links.push(item);
                    }
                }
            }
            for link in &links {
                paint_call_underline(
                    ui,
                    body_origin,
                    link,
                    hscroll,
                    char_w,
                    row_h,
                    visible_cols,
                    text_color,
                );
            }

            if current && focus == Focus::Data {
                let screen_col = cursor.1.saturating_sub(1).saturating_sub(hscroll);
                if screen_col < visible_cols {
                    let ch = chars.get(screen_col).copied().unwrap_or(' ');
                    let on_find = self
                        .view
                        .shown_find_spans(row.line)
                        .iter()
                        .any(|(col, len)| cursor.1 >= *col && cursor.1 < col + len);
                    let color = if on_find {
                        FIND_TEXT
                    } else {
                        kinds
                            .get(screen_col)
                            .copied()
                            .map(|kind| kind_color(kind, text_color, &colors))
                            .unwrap_or(text_color)
                    };
                    paint_cursor(
                        ui,
                        Pos2::new(body_origin.x + screen_col as f32 * char_w, rect.min.y),
                        ch,
                        &font,
                        char_w,
                        row_h,
                        self.view.insert_mode(),
                        color,
                    );
                }
            }
            if current && focus == Focus::Prefix {
                let screen_col = self.view.prefix_col().saturating_sub(1).min(7);
                let ch = prefixes[index].1.chars().nth(screen_col).unwrap_or(' ');
                paint_cursor(
                    ui,
                    Pos2::new(rect.min.x + screen_col as f32 * char_w, rect.min.y),
                    ch,
                    &font,
                    char_w,
                    row_h,
                    self.view.insert_mode(),
                    prefix_color,
                );
            }

            if response.secondary_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let x = pos.x - rect.min.x;
                    self.click_row(row.line, x, prefix_w, gap, char_w);
                    self.editor_menu = None;
                    if !sentinel && !exclude && x >= prefix_w + gap {
                        let col = ((x - prefix_w - gap) / char_w).floor() as usize + 1 + hscroll;
                        let tab = self.view.session.current();
                        let folder = tab.path.as_ref().and_then(|path| path.parent());
                        let index = name_index(&tab.doc, folder);
                        if let Some(link) = definition_at(
                            shown.as_ref(),
                            row.line,
                            col,
                            &index.variables,
                            &index.paragraphs,
                            &index.copies,
                        ) {
                            let start = link.col.saturating_sub(1);
                            let menu_x =
                                body_origin.x + start.saturating_sub(hscroll) as f32 * char_w;
                            let menu_y = body_origin.y + row_h;
                            self.tab_menu = None;
                            self.editor_menu = Some(EditorMenu {
                                pos: Pos2::new(menu_x, menu_y),
                                line: link.line,
                                col: link.target_col,
                                path: link.path,
                            });
                            self.editor_menu_fresh = true;
                            let _ = pos;
                        }
                    }
                }
            } else if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let x = pos.x - rect.min.x;
                    if link_ctrl && x >= prefix_w + gap {
                        let col = ((x - prefix_w - gap) / char_w).floor() as usize + 1 + hscroll;
                        if let Some(link) = links
                            .iter()
                            .find(|link| col >= link.col && col < link.col + link.len)
                        {
                            self.attention_used = true;
                            self.click_row(row.line, x, prefix_w, gap, char_w);
                            self.follow_call(link);
                        } else {
                            self.click_row(row.line, x, prefix_w, gap, char_w);
                        }
                    } else {
                        self.click_row(row.line, x, prefix_w, gap, char_w);
                    }
                }
            }
        }
        show_editor_menu(ui, self);

        // A short last page must still occupy the editor, or the status bar
        // follows the last row upward.
        if self.crosshair && matches!(focus, Focus::Data | Focus::Prefix) {
            if let Some(slot) = (self.top_row..end).position(|index| rows[index].line == cursor.0) {
                let stroke = Stroke::new(1.0, Color32::from_rgb(180, 220, 255));
                let y = editor_origin.y + (cols_rows as f32 + slot as f32 + 1.0) * row_h;
                ui.painter()
                    .hline(editor_origin.x..=(editor_origin.x + data_width), y, stroke);
                let x = if focus == Focus::Prefix {
                    let screen_col = self.view.prefix_col().saturating_sub(1).min(7);
                    Some(editor_origin.x + screen_col as f32 * char_w)
                } else {
                    let screen_col = cursor.1.saturating_sub(1).saturating_sub(hscroll);
                    (screen_col < visible_cols)
                        .then_some(editor_origin.x + prefix_w + gap + screen_col as f32 * char_w)
                };
                if let Some(x) = x {
                    self.crosshair_x = Some(x);
                }
            }
        }
        let shown = end.saturating_sub(self.top_row) + cols_rows;
        let rest = (height - shown as f32 * row_h).max(0.0);
        if rest > 0.0 {
            let _ = ui.allocate_exact_size(Vec2::new(data_width, rest), Sense::hover());
        }
    }

    fn click_row(&mut self, line: usize, x: f32, prefix_w: f32, gap: f32, char_w: f32) {
        self.swap_point = false;
        if x < prefix_w {
            let col = (x / char_w).floor() as usize + 1;
            self.view.click(PointerTarget::Prefix { line, col });
        } else if x >= prefix_w + gap {
            let col = ((x - prefix_w - gap) / char_w).floor() as usize + 1 + self.hscroll;
            self.view.click(PointerTarget::Data { line, col });
        }
    }

    fn can_jump_back(&self) -> bool {
        self.jump_at > 0
    }

    fn can_jump_forward(&self) -> bool {
        self.jump_at + 1 < self.jumps.len()
    }

    /// Ctrl-click on a paragraph, variable, or copybook. The place being left is kept so Back can return.
    fn follow_call(&mut self, link: &ParagraphCall) {
        if self.jump_at < self.jumps.len() {
            self.jumps.truncate(self.jump_at + 1);
        }
        let tab = self.view.session.current().id;
        let (line, col_here) = self.view.cursor();
        let same = self
            .jumps
            .get(self.jump_at)
            .is_some_and(|item| item.tab == tab && item.line == line && item.col == col_here);
        if same {
            self.remember_scroll();
        } else {
            self.jumps.push(self.place_at_cursor());
        }
        if let Some(path) = link.path.as_ref() {
            self.open_paths(vec![path.clone()]);
        }
        let col = link.target_col.max(1);
        self.view.jump_to_line(link.line, col);
        let mut place = self.place_at_cursor();
        place.top = top_for_second_line(
            self.view
                .session
                .current()
                .doc
                .visible_rows()
                .iter()
                .position(|row| row.line == place.line)
                .unwrap_or(0),
        );
        self.top_row = place.top;
        self.hscroll = place.left;
        self.jumps.push(place);
        self.jump_at = self.jumps.len() - 1;
    }

    fn jump_back(&mut self) {
        if !self.can_jump_back() {
            return;
        }
        self.remember_scroll();
        self.jump_at -= 1;
        self.restore_jump(self.jumps[self.jump_at].clone());
    }

    fn jump_forward(&mut self) {
        if !self.can_jump_forward() {
            return;
        }
        self.remember_scroll();
        self.jump_at += 1;
        self.restore_jump(self.jumps[self.jump_at].clone());
    }

    /// Keep the scroll of the current history place when the cursor is still there.
    fn remember_scroll(&mut self) {
        let tab = self.view.session.current().id;
        let (line, col) = self.view.cursor();
        let Some(place) = self.jumps.get_mut(self.jump_at) else {
            return;
        };
        if place.tab == tab && place.line == line && place.col == col {
            place.top = self.top_row;
            place.left = self.hscroll;
        }
    }

    fn place_at_cursor(&self) -> JumpPlace {
        let (line, col) = self.view.cursor();
        let rows = self.view.session.current().doc.visible_rows();
        let idx = rows.iter().position(|row| row.line == line).unwrap_or(0);
        let top = revealed_top(self.top_row, idx, self.page_rows);
        let left = revealed_left(
            self.hscroll,
            col.saturating_sub(1),
            self.visible_cols.max(1),
        );
        JumpPlace {
            tab: self.view.session.current().id,
            line,
            col,
            top,
            left,
        }
    }

    /// Move to a recorded place and restore its scroll when that window still shows the cursor.
    fn restore_jump(&mut self, place: JumpPlace) {
        if self.view.session.current().id != place.tab {
            let _ = self.select_tab(place.tab);
        }
        self.view.jump_to_line(place.line, place.col);
        let line = self.view.cursor().0;
        let rows = self.view.session.current().doc.visible_rows();
        let idx = rows.iter().position(|row| row.line == line);
        if scroll_can_show(place.top, idx, self.page_rows, rows.len()) {
            self.top_row = place.top;
            self.hscroll = place.left;
        }
    }

    fn apply_pending_close(&mut self, ctx: &egui::Context) {
        match self.view.take_pending_close() {
            Some(PendingClose::Discard) => {
                self.exit_after_save = false;
                let last = self.view.session.tabs().len() == 1;
                self.finish_end(ctx, last);
            }
            Some(PendingClose::Save) => {
                if self.view.session.current().path.is_none() {
                    self.exit_after_save = true;
                    self.save_as_dialog();
                } else {
                    self.save_current();
                }
                if self.view.session.current().path.is_none()
                    || self.view.session.current().doc.is_modified()
                {
                    self.exit_after_save = false;
                    return;
                }
                self.exit_after_save = false;
                let last = self.view.session.tabs().len() == 1;
                self.finish_end(ctx, last);
            }
            None => {}
        }
    }

    fn reveal_column(&mut self, visible_cols: usize) {
        if self.view.focus() != Focus::Data {
            return;
        }
        let col0 = self.view.cursor().1.saturating_sub(1);
        self.hscroll = revealed_left(self.hscroll, col0, visible_cols);
    }

    /// ISPF SWAPBAR: the last line lists every screen. `*` is active, `-` is the alternate.
    fn paint_swapbar(&mut self, ui: &mut egui::Ui, font: &FontId, char_w: f32, row_h: f32) {
        let names: Vec<String> = self
            .view
            .session
            .tabs()
            .iter()
            .map(|tab| tab.title.clone())
            .collect();
        let current_id = self.view.session.current().id;
        let current = self
            .view
            .session
            .tabs()
            .iter()
            .position(|tab| tab.id == current_id)
            .unwrap_or(0);
        let alternate = self.view.session.alternate();
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
        let fields = swapbar_fields(&borrowed, current, alternate);
        let cols = (ui.available_width() / char_w).floor().max(1.0) as usize;
        let window = swapbar_window(fields.len(), self.swap_from, cols);
        self.swap_from = window.from;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::CLICK);
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(0, 70, 140));
        let mut col = usize::from(window.left);
        if window.left {
            paint_string(
                ui,
                rect.min,
                "<",
                font,
                Color32::from_rgb(255, 255, 120),
                char_w,
            );
        }
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        for (slot, field) in fields
            .iter()
            .skip(window.from)
            .take(window.shown)
            .enumerate()
        {
            let body = field
                .label
                .split_once(':')
                .map(|(_, rest)| rest)
                .unwrap_or(field.label.as_str());
            let color = if body.starts_with('*') {
                Color32::from_rgb(255, 255, 120)
            } else if body.starts_with('-') {
                Color32::from_rgb(180, 230, 255)
            } else {
                Color32::WHITE
            };
            let width = field.label.chars().count();
            paint_string(
                ui,
                Pos2::new(rect.min.x + col as f32 * char_w, rect.min.y),
                &field.label,
                font,
                color,
                char_w,
            );
            hits.push((col, col + width, field.index));
            col += if slot + 1 == window.shown {
                width
            } else {
                width + 1
            };
        }
        let right_col = col;
        if window.right {
            paint_string(
                ui,
                Pos2::new(rect.min.x + right_col as f32 * char_w, rect.min.y),
                ">",
                font,
                Color32::from_rgb(255, 255, 120),
                char_w,
            );
        }
        if response.clicked() {
            self.swap_point = true;
            if let Some(pos) = response.interact_pointer_pos() {
                let xcol = ((pos.x - rect.min.x) / char_w).floor().max(0.0) as usize;
                if window.left && xcol == 0 {
                    self.swap_from = self.swap_from.saturating_sub(1);
                } else if window.right && xcol >= right_col {
                    let last = fields.len().saturating_sub(1);
                    self.swap_from = (self.swap_from + 1).min(last);
                } else if let Some((_, _, index)) = hits
                    .iter()
                    .find(|(start, end, _)| xcol >= *start && xcol < *end)
                {
                    if *index == current {
                        self.swap_point = false;
                        self.view.handle_key(EditorKey::Enter);
                    } else {
                        self.activate_tab(*index);
                    }
                }
            }
        }
    }

    fn activate_tab(&mut self, index: usize) {
        if self.view.session.select(index).is_err() {
            return;
        }
        self.show_swapped_screen();
    }

    fn reveal_if_swapped(&mut self) {
        if !self.view.session.take_pending_reveal() {
            return;
        }
        self.show_swapped_screen();
    }

    /// Show the screen that was just swapped in, with the cursor on the command line.
    fn show_swapped_screen(&mut self) {
        self.swap_point = false;
        self.hscroll = 0;
        self.top_row = 0;
        let (line, col) = self.view.cursor();
        self.view.jump_to_line(line, col);
        self.view.click(PointerTarget::Command { col: 1 });
    }

    fn start_new_screen(&mut self) {
        self.view.session.add_screen();
        self.show_swapped_screen();
        let _ = self.view.session.take_pending_reveal();
    }

    fn swap_list_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut pick = None;
        let mut start_new = false;
        let current = self.view.session.current_index();
        let rows: Vec<(usize, String)> = self
            .view
            .session
            .tabs()
            .iter()
            .enumerate()
            .map(|(index, tab)| (index, tab.title.clone()))
            .collect();
        if self.swap_list_at.is_none() {
            self.swap_list_at = Some(current.min(rows.len()));
        }
        let chosen = self.swap_list_at.unwrap_or(current).min(rows.len());
        egui::Window::new("Document List")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Select a screen.");
                for (index, title) in &rows {
                    let mark = if *index == current { '*' } else { ' ' };
                    let label = format!("{mark}{:>3}  {title}", index + 1);
                    let response = ui.selectable_label(chosen == *index, label);
                    if chosen == *index {
                        hold_dialog_focus(&response);
                    }
                    if response.clicked() {
                        pick = Some(*index);
                    }
                }
                ui.separator();
                let start = ui.selectable_label(chosen == rows.len(), "Start a new screen");
                if chosen == rows.len() {
                    hold_dialog_focus(&start);
                }
                if start.clicked() {
                    start_new = true;
                }
            });
        if start_new {
            self.start_new_screen();
            self.view.session.set_swap_list(false);
            self.swap_list_at = None;
        } else if let Some(index) = pick {
            let _ = self.view.session.activate_screen(index);
            self.view.session.set_swap_list(false);
            self.swap_list_at = None;
        } else if !open {
            self.view.session.set_swap_list(false);
            self.swap_list_at = None;
        }
    }

    /// Vertical crosshair from the top of the command line through the editor.
    fn paint_crosshair_span(&self, ui: &egui::Ui) {
        let (Some(x), Some(command), Some(editor)) =
            (self.crosshair_x, self.command_bar, self.editor_bar)
        else {
            return;
        };
        let stroke = Stroke::new(1.0, Color32::from_rgb(180, 220, 255));
        ui.painter()
            .vline(x, command.top()..=editor.bottom(), stroke);
    }

    /// Remember how far the editor sits to the right of the command line.
    fn sync_cross_layout(&mut self, char_w: f32) {
        let shift = match (self.command_bar, self.editor_bar) {
            (Some(command), Some(editor)) if char_w > 0.0 => {
                ((editor.min.x - command.min.x) / char_w).round() as isize
            }
            _ => 0,
        };
        self.view
            .set_cross_layout(shift, self.hscroll, self.visible_cols.max(1));
    }

    fn paint_command(&mut self, ui: &mut egui::Ui, font: &FontId, char_w: f32, row_h: f32) {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), row_h),
            Sense::CLICK | Sense::DRAG,
        );
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 28));
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(1.0, Color32::from_rgb(40, 70, 120)),
        );
        let prompt = "Command> ";
        paint_string(
            ui,
            rect.min,
            prompt,
            font,
            Color32::from_rgb(255, 214, 80),
            char_w,
        );
        let prompt_cols = prompt.chars().count();
        let body = Pos2::new(rect.min.x + prompt_cols as f32 * char_w, rect.min.y);
        let message = status_message(&self.view);
        let width_cols = (rect.width() / char_w).floor() as usize;
        let message_cols = message
            .chars()
            .count()
            .min(width_cols.saturating_sub(prompt_cols + 2));
        let message = fit_right(message, message_cols);
        let message_cols = message.chars().count();
        let command_limit = width_cols
            .saturating_sub(prompt_cols + message_cols + 2)
            .max(1);
        self.view.set_command_field_width(command_limit);
        let scroll = self.view.command_scroll();
        let chars: Vec<char> = self.view.command().chars().skip(scroll).collect();
        paint_chars(
            ui,
            body,
            &chars,
            &[],
            font,
            Color32::from_rgb(236, 236, 236),
            &syntax_colors(ColorTheme::Current),
            char_w,
            command_limit,
        );
        if let Some((start, end)) = self.view.command_selection() {
            paint_find_marks(
                ui,
                body,
                &[(start, end.saturating_sub(start))],
                &chars,
                font,
                scroll,
                char_w,
                row_h,
                command_limit,
            );
        }
        if message_cols > 0 {
            let origin = Pos2::new(rect.max.x - message_cols as f32 * char_w - 4.0, rect.min.y);
            paint_string(
                ui,
                origin,
                &message,
                font,
                Color32::from_rgb(255, 196, 120),
                char_w,
            );
        }
        if self.view.focus() == Focus::Command {
            let screen_col = self
                .view
                .command_col()
                .saturating_sub(1)
                .saturating_sub(scroll);
            if screen_col < command_limit {
                let ch = chars.get(screen_col).copied().unwrap_or(' ');
                paint_cursor(
                    ui,
                    Pos2::new(body.x + screen_col as f32 * char_w, rect.min.y),
                    ch,
                    font,
                    char_w,
                    row_h,
                    self.view.insert_mode(),
                    Color32::from_rgb(236, 236, 236),
                );
            }
        }
        self.command_bar = Some(rect);
        if self.crosshair && self.view.focus() == Focus::Command {
            let stroke = Stroke::new(1.0, Color32::from_rgb(180, 220, 255));
            let y = rect.min.y + row_h;
            ui.painter().hline(rect.x_range(), y, stroke);
            let screen_col = self
                .view
                .command_col()
                .saturating_sub(1)
                .saturating_sub(scroll);
            if screen_col < command_limit {
                self.crosshair_x = Some(body.x + screen_col as f32 * char_w);
            }
        }
        if response.clicked() || response.dragged() {
            self.swap_point = false;
            if let Some(pos) = response.interact_pointer_pos() {
                let x = (pos.x - body.x).max(0.0);
                let col = (x / char_w).floor() as usize + 1 + scroll;
                self.view.click(PointerTarget::Command { col });
            }
        }
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if (ui.ctx().zoom_factor() - 1.0).abs() > 0.001 {
            ui.ctx().set_zoom_factor(1.0);
        }
        let closing = ui.input(|input| input.viewport().close_requested());
        if closing && self.view.session.current().doc.is_modified() && !self.allow_close {
            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
            self.close_prompt = true;
        } else if closing && !self.exiting {
            self.return_open_files_to_recent();
            self.save_config();
            self.exiting = true;
        }

        self.handle_input(ui);
        self.apply_pending_close(ui.ctx());
        let menu_open = egui::Popup::is_any_open(ui.ctx());
        if !self.dialog_open() && !self.view.session.swap_list_open() && !menu_open {
            ui.ctx().memory_mut(|mem| {
                if let Some(id) = mem.focused() {
                    mem.surrender_focus(id);
                }
            });
        }
        self.reveal_if_swapped();
        let title = window_title(&self.view);
        ui.ctx().send_viewport_cmd(ViewportCommand::Title(title));

        let mut frame = egui::Frame::central_panel(&ui.style());
        frame.inner_margin.bottom = 0;
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            menu_bar(ui, self);
            if self.show_toolbar {
                toolbar(ui, self);
            }
            tab_strip(ui, self);
            self.command_bar = None;
            self.editor_bar = None;
            self.outline_rect = None;
            self.crosshair_x = None;
            let command_px = self.command_px();
            let editor_px = self.editor_px();
            let font = FontId::monospace(command_px);
            let editor_font = FontId::monospace(editor_px);
            let chrome = FontId::monospace(FONT_SIZE);
            let (char_w, row_h) = ui.painter().fonts_mut(|fonts| {
                (
                    fonts.glyph_width(&font, 'M').max(1.0),
                    fonts.row_height(&font).max(command_px),
                )
            });
            let editor_char_w = ui
                .painter()
                .fonts_mut(|fonts| fonts.glyph_width(&editor_font, 'M').max(1.0));
            let editor_row_h = ui
                .painter()
                .fonts_mut(|fonts| fonts.row_height(&editor_font).max(editor_px));
            let (chrome_w, chrome_h) = ui.painter().fonts_mut(|fonts| {
                (
                    fonts.glyph_width(&chrome, 'M').max(1.0),
                    fonts.row_height(&chrome).max(FONT_SIZE),
                )
            });
            self.paint_command(ui, &font, char_w, row_h);
            ui.spacing_mut().item_spacing.y = 0.0;
            let bar_h = if self.show_status_bar {
                status_bar_height(chrome_h)
            } else {
                0.0
            };
            let swap_h = if self.view.session.swapbar() {
                chrome_h
            } else {
                0.0
            };
            let editor_h = (ui.available_height() - bar_h - swap_h).max(0.0);
            if editor_h >= editor_row_h {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), editor_h),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        self.paint_outline(ui, editor_h);
                        ui.allocate_ui_with_layout(
                            Vec2::new(ui.available_width(), editor_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| self.paint_editor(ui),
                        );
                    },
                );
                self.paint_crosshair_span(ui);
                self.sync_cross_layout(editor_char_w);
            }
            if self.show_status_bar {
                status_line(ui, &self.view, &chrome, chrome_w, chrome_h);
            }
            if self.view.session.swapbar() {
                self.paint_swapbar(ui, &chrome, chrome_w, chrome_h);
            }
        });

        if self.close_prompt {
            self.close_dialog(ui.ctx());
        }
        if self.options_open {
            self.options_dialog(ui.ctx());
        }
        if self.recent_open {
            self.recent_dialog(ui.ctx());
        }
        if self.help_open {
            self.help_dialog(ui.ctx());
        }
        if self.about_open {
            self.about_dialog(ui.ctx());
        }
        if self.end_prompt {
            self.end_dialog(ui.ctx());
        }
        if self.reopen_prompt {
            self.reopen_dialog(ui.ctx());
        }
        if self.view.session.swap_list_open() {
            self.swap_list_dialog(ui.ctx());
            self.reveal_if_swapped();
        }
        if !self.exiting {
            self.conceal_open_files();
        }
        if self.front_dialog().is_none() {
            self.dialog_focus = None;
        }
        self.dialog_confirm = false;
    }
}

impl Window {
    fn close_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("ISPFLite")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This file has unsaved changes.");
                ui.horizontal(|ui| {
                    let save = ui.button("Save");
                    self.grab_focus(DialogKind::Close, &save);
                    if self.activated(&save) {
                        self.save_current();
                        if !self.view.session.current().doc.is_modified() {
                            self.allow_close = true;
                            self.close_prompt = false;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    }
                    let discard = ui.button("Don't save");
                    if self.activated(&discard) {
                        self.allow_close = true;
                        self.close_prompt = false;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    let cancel = ui.button("Cancel");
                    if self.activated(&cancel) {
                        self.close_prompt = false;
                    }
                });
            });
    }

    fn end_file(&mut self, ctx: &egui::Context) {
        let last = self.view.session.tabs().len() == 1;
        if self.view.session.current().doc.is_modified() {
            self.end_prompt = true;
            self.end_closes_window = last;
            return;
        }
        self.finish_end(ctx, last);
    }

    fn finish_end(&mut self, ctx: &egui::Context, close_window: bool) {
        if close_window {
            self.return_open_files_to_recent();
            self.save_config();
            self.exiting = true;
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let path = self.view.session.current().path.clone();
        self.view.drop_tab();
        self.hscroll = 0;
        self.top_row = 0;
        self.release_if_unused(path);
    }

    fn end_dialog(&mut self, ctx: &egui::Context) {
        let close_window = self.end_closes_window;
        egui::Window::new("ISPFLite")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This file has unsaved changes.");
                ui.horizontal(|ui| {
                    let save = ui.button("Save");
                    self.grab_focus(DialogKind::End, &save);
                    if self.activated(&save) {
                        self.save_current();
                        if !self.view.session.current().doc.is_modified() {
                            self.end_prompt = false;
                            self.finish_end(ctx, close_window);
                        }
                    }
                    let discard = ui.button("Don't save");
                    if self.activated(&discard) {
                        self.end_prompt = false;
                        self.finish_end(ctx, close_window);
                    }
                    let cancel = ui.button("Cancel");
                    if self.activated(&cancel) {
                        self.end_prompt = false;
                    }
                });
            });
    }

    fn reopen_current(&mut self) {
        if self.view.session.current().path.is_none() {
            self.view.set_status("No file to re-open");
            return;
        }
        if self.view.session.current().doc.is_modified() {
            self.reopen_prompt = true;
            return;
        }
        self.finish_reopen();
    }

    fn finish_reopen(&mut self) {
        self.reopen_prompt = false;
        if self.view.reopen_current() {
            self.hscroll = 0;
            self.top_row = 0;
            self.jumps.clear();
            self.jump_at = 0;
        }
    }

    fn reopen_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Re-open")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This file has unsaved changes. Re-open discards them and loads the file again.");
                ui.horizontal(|ui| {
                    let reopen = ui.button("Re-open");
                    self.grab_focus(DialogKind::Reopen, &reopen);
                    if self.activated(&reopen) {
                        self.finish_reopen();
                    }
                    let cancel = ui.button("Cancel");
                    if self.activated(&cancel) {
                        self.reopen_prompt = false;
                    }
                });
            });
    }

    fn help_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.help_open;
        let mut close = false;
        egui::Window::new("Help")
            .collapsible(false)
            .resizable(true)
            .default_width(680.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        let delta = ui.input(|input| {
                            let mut y = 0.0;
                            if input.key_pressed(Key::ArrowDown) {
                                y -= 18.0;
                            }
                            if input.key_pressed(Key::ArrowUp) {
                                y += 18.0;
                            }
                            if input.key_pressed(Key::PageDown) {
                                y -= 320.0;
                            }
                            if input.key_pressed(Key::PageUp) {
                                y += 320.0;
                            }
                            egui::vec2(0.0, y)
                        });
                        if delta.y != 0.0 {
                            ui.scroll_with_delta(delta);
                        }
                        ui.monospace(help_text());
                    });
                let ok = ui.button("OK");
                self.grab_focus(DialogKind::Help, &ok);
                ok.ctx.memory_mut(|mem| {
                    mem.set_focus_lock_filter(
                        ok.id,
                        egui::EventFilter {
                            tab: false,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
                if self.activated(&ok) {
                    close = true;
                }
            });
        self.help_open = open && !close;
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.about_open;
        let mut close = false;
        egui::Window::new("About")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                ui.vertical_centered(|ui| {
                    ui.label(format!("ISPFLite v{VERSION}"));
                    ui.label("By KC Wong via Grok AI ");
                    ui.label("Nov 2026");
                    ui.add_space(8.0);
                    let ok = ui.button("OK");
                    self.grab_focus(DialogKind::About, &ok);
                    if self.activated(&ok) {
                        close = true;
                    }
                });
            });
        self.about_open = open && !close;
    }

    fn options_dialog(&mut self, ctx: &egui::Context) {
        if self.options_draft.is_none() {
            self.options_draft = Some(OptionsDraft {
                enter_key: self.enter_key,
                next_line_at: self.next_line_at,
                color_theme: self.color_theme,
                recent_limit: self.recent_limit,
            });
        }
        let Some(mut draft) = self.options_draft.take() else {
            return;
        };
        let mut open = self.options_open;
        let mut accept = false;
        let mut cancel = false;
        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                const DROPDOWN_WIDTH: f32 = 240.0;
                let grid = egui::Grid::new("options")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Enter function");
                        let enter = egui::ComboBox::from_id_salt("enter_function")
                            .selected_text(draft.enter_key.label())
                            .width(DROPDOWN_WIDTH)
                            .show_ui(ui, |ui| {
                                for choice in EnterKey::CHOICES {
                                    ui.selectable_value(
                                        &mut draft.enter_key,
                                        choice,
                                        choice.label(),
                                    );
                                }
                            });
                        self.grab_focus(DialogKind::Options, &enter.response);
                        ui.end_row();

                        ui.label("Next line cursor location");
                        egui::ComboBox::from_id_salt("next_line_cursor")
                            .selected_text(next_line_label(draft.next_line_at))
                            .width(DROPDOWN_WIDTH)
                            .show_ui(ui, |ui| {
                                for choice in [NextLineCursor::LineNumber, NextLineCursor::Text] {
                                    ui.selectable_value(
                                        &mut draft.next_line_at,
                                        choice,
                                        next_line_label(choice),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Syntax highlight");
                        egui::ComboBox::from_id_salt("syntax_theme")
                            .selected_text(draft.color_theme.label())
                            .width(DROPDOWN_WIDTH)
                            .show_ui(ui, |ui| {
                                for choice in ColorTheme::CHOICES {
                                    ui.selectable_value(
                                        &mut draft.color_theme,
                                        choice,
                                        choice.label(),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Recent files");
                        ui.add(egui::DragValue::new(&mut draft.recent_limit).range(1..=50));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(grid.response.rect.width(), 28.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.horizontal(|ui| {
                            let ok = ui.button("OK");
                            if self.activated(&ok) {
                                accept = true;
                            }
                            let cancel_button = ui.button("Cancel");
                            if self.activated(&cancel_button) {
                                cancel = true;
                            }
                        });
                    },
                );
            });
        if accept {
            let changed = self.enter_key != draft.enter_key
                || self.next_line_at != draft.next_line_at
                || self.color_theme != draft.color_theme
                || self.recent_limit != draft.recent_limit;
            self.enter_key = draft.enter_key;
            self.next_line_at = draft.next_line_at;
            self.color_theme = draft.color_theme;
            self.recent_limit = draft.recent_limit;
            self.recent.truncate(self.recent_limit.max(1));
            self.view.refresh_empty_status(self.enter_key.label());
            if changed {
                self.save_config();
            }
            self.options_open = false;
        } else if cancel || !open {
            self.options_draft = None;
            self.options_open = false;
        } else {
            self.options_draft = Some(draft);
        }
    }

    /// Close the front dialog. Escape uses this for every dialog.
    fn dialog_open(&self) -> bool {
        self.front_dialog().is_some()
    }

    fn front_dialog(&self) -> Option<DialogKind> {
        if self.close_prompt {
            Some(DialogKind::Close)
        } else if self.end_prompt {
            Some(DialogKind::End)
        } else if self.reopen_prompt {
            Some(DialogKind::Reopen)
        } else if self.options_open {
            Some(DialogKind::Options)
        } else if self.help_open {
            Some(DialogKind::Help)
        } else if self.about_open {
            Some(DialogKind::About)
        } else if self.recent_open {
            Some(DialogKind::Recent)
        } else {
            None
        }
    }

    fn wants_dialog_focus(&self, kind: DialogKind) -> bool {
        self.front_dialog() == Some(kind) && self.dialog_focus != Some(kind)
    }

    fn grab_focus(&mut self, kind: DialogKind, response: &egui::Response) {
        if self.wants_dialog_focus(kind) {
            response.request_focus();
            self.dialog_focus = Some(kind);
        }
    }

    /// A focused dialog control was clicked, or the editor's Enter key confirmed it.
    fn activated(&mut self, response: &egui::Response) -> bool {
        if response.clicked() {
            self.dialog_confirm = false;
            return true;
        }
        if self.dialog_confirm && response.has_focus() {
            self.dialog_confirm = false;
            return true;
        }
        false
    }

    fn dismiss_dialog(&mut self) -> bool {
        if self.close_prompt {
            self.close_prompt = false;
            return true;
        }
        if self.end_prompt {
            self.end_prompt = false;
            return true;
        }
        if self.reopen_prompt {
            self.reopen_prompt = false;
            return true;
        }
        if self.options_open {
            self.options_open = false;
            self.options_draft = None;
            return true;
        }
        if self.help_open {
            self.help_open = false;
            return true;
        }
        if self.about_open {
            self.about_open = false;
            return true;
        }
        if self.recent_open {
            self.recent_open = false;
            return true;
        }
        if self.view.session.swap_list_open() {
            self.view.session.set_swap_list(false);
            self.swap_list_at = None;
            return true;
        }
        false
    }

    fn recent_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.recent_open;
        let paths = self.recent.clone();
        let mut pick = None;
        egui::Window::new("Recent files")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                if paths.is_empty() {
                    ui.label("No recent files.");
                    let idle = ui.allocate_response(
                        egui::Vec2::ZERO,
                        egui::Sense::focusable_noninteractive(),
                    );
                    self.grab_focus(DialogKind::Recent, &idle);
                }
                for (index, path) in paths.iter().enumerate() {
                    let button = ui.button(path.display().to_string());
                    if index == 0 {
                        self.grab_focus(DialogKind::Recent, &button);
                    }
                    if self.activated(&button) {
                        pick = Some(path.clone());
                    }
                }
            });
        self.recent_open = open;
        if let Some(path) = pick {
            self.open_paths(vec![path]);
            self.recent_open = false;
        }
    }
}

fn hover_note(response: &egui::Response, text: &str) {
    let over = response.contains_pointer()
        || response.ctx.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pos| response.rect.contains(pos))
        });
    if !over {
        return;
    }
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 3))
        .fill(Color32::from_rgb(255, 255, 224))
        .stroke(Stroke::new(1.0, Color32::from_rgb(176, 160, 64)))
        .corner_radius(2);
    egui::Popup::from_response(response)
        .kind(egui::PopupKind::Tooltip)
        .sense(Sense::hover())
        .interactable(false)
        .gap(4.0)
        .frame(frame)
        .show(|ui| {
            ui.style_mut().interaction.selectable_labels = false;
            ui.visuals_mut().override_text_color = Some(Color32::BLACK);
            ui.label(egui::RichText::new(text).color(Color32::BLACK));
        });
}

fn menu_tick(shown: bool) -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(14.0), move |ui, args| {
        if shown {
            icon_line(
                ui.painter(),
                args.rect,
                &[(2.4, 7.6), (6.2, 12.0), (13.4, 3.2)],
                Color32::from_rgb(88, 112, 156),
            );
        }
    })
}

fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: Option<&str>,
    focus_first: &mut bool,
) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(shortcut);
    }
    let response = ui.add(button);
    if *focus_first {
        response.request_focus();
        *focus_first = false;
    }
    let clicked = response.clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn menu_check(ui: &mut egui::Ui, label: &str, shown: bool, focus_first: &mut bool) -> bool {
    let response = ui.add(egui::Button::new((menu_tick(shown), label)));
    if *focus_first {
        response.request_focus();
        *focus_first = false;
    }
    let clicked = response.clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn menu_bar(ui: &mut egui::Ui, window: &mut Window) {
    let underline = window.alt_held;
    let pending = window.pending_menu.take();
    let mut opened = false;
    let active = window.active_top_menu;
    let descend = window.menu_descend;
    window.menu_descend = false;
    egui::MenuBar::new().ui(ui, |ui| {
        let mut focus_first = descend && active == Some('F');
        opened |= top_menu(ui, "File", underline, pending == Some('F'), |ui| {
            if menu_item(ui, "New", Some("Ctrl+N"), &mut focus_first) {
                window.start_new_screen();
            }
            if menu_item(ui, "Open", Some("Ctrl+O"), &mut focus_first) {
                window.open_dialog();
            }
            if menu_item(ui, "Re-open", Some("Ctrl+R"), &mut focus_first) {
                window.reopen_current();
            }
            if menu_item(ui, "Open Recent", Some("Ctrl+Shift+R"), &mut focus_first) {
                window.recent_open = true;
            }
            ui.separator();
            if menu_item(ui, "Save", Some("Ctrl+S"), &mut focus_first) {
                window.save_current();
            }
            if menu_item(ui, "Save As", Some("Ctrl+Shift+S"), &mut focus_first) {
                window.save_as_dialog();
            }
            ui.separator();
            if menu_item(ui, "Settings", Some("Ctrl+Shift+O"), &mut focus_first) {
                window.options_open = true;
            }
            ui.separator();
            if menu_item(ui, "Close", Some("Ctrl+W"), &mut focus_first) {
                window.end_file(ui.ctx());
            }
            if menu_item(ui, "Exit", None, &mut focus_first) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        let mut focus_first = descend && active == Some('E');
        opened |= top_menu(ui, "Edit", underline, pending == Some('E'), |ui| {
            if menu_item(ui, "Cut", Some("Ctrl+X"), &mut focus_first) {
                window.cut_selection(ui.ctx());
            }
            if menu_item(ui, "Copy", Some("Ctrl+C"), &mut focus_first) {
                window.copy_selection(ui.ctx());
            }
            if menu_item(ui, "Paste", Some("Ctrl+V"), &mut focus_first) {
                window.paste_from_clipboard();
            }
            ui.separator();
            if menu_item(ui, "Select All", Some("Ctrl+A"), &mut focus_first) {
                window.view.select_all();
            }
        });
        let mut focus_first = descend && active == Some('V');
        opened |= top_menu(ui, "View", underline, pending == Some('V'), |ui| {
            let zoom = ui.menu_button("Zoom", |ui| {
                let mut nested = false;
                if menu_item(ui, "Zoom In", Some("Ctrl+Plus"), &mut nested) {
                    window.zoom_in();
                }
                if menu_item(ui, "Zoom Out", Some("Ctrl+Minus"), &mut nested) {
                    window.zoom_out();
                }
                if menu_item(ui, "Restore Default Zoom", Some("Ctrl+0"), &mut nested) {
                    window.zoom_reset();
                }
            });
            if focus_first {
                zoom.response.request_focus();
                focus_first = false;
            }
            ui.separator();
            if menu_check(ui, "Status Bar", window.show_status_bar, &mut focus_first) {
                window.show_status_bar = !window.show_status_bar;
                window.save_config();
            }
            if menu_check(ui, "Tool Bar", window.show_toolbar, &mut focus_first) {
                window.show_toolbar = !window.show_toolbar;
                window.save_config();
            }
            if menu_check(
                ui,
                "Outline Panel",
                window.outline_width > 0.0,
                &mut focus_first,
            ) {
                window.toggle_outline();
            }
        });
        let mut focus_first = descend && active == Some('W');
        opened |= top_menu(ui, "Window", underline, pending == Some('W'), |ui| {
            let current_id = window.view.session.current().id;
            let tabs: Vec<(u32, String)> = window
                .view
                .session
                .tabs()
                .iter()
                .map(|tab| (tab.id, tab.title.clone()))
                .collect();
            for (index, (id, title)) in tabs.iter().enumerate() {
                let label = format!("{}: {title}", index + 1);
                if menu_check(ui, &label, *id == current_id, &mut focus_first) {
                    let _ = window.select_tab(*id);
                }
            }
        });
        let mut focus_first = descend && active == Some('H');
        opened |= top_menu(ui, "Help", underline, pending == Some('H'), |ui| {
            if menu_item(ui, "About", None, &mut focus_first) {
                window.about_open = true;
            }
        });
    });
    if opened {
        // Wait for Alt / Left / Right to go up before selecting the first item.
        window.menu_select_pending = true;
        window.menu_descend = false;
        if let Some(menu) = pending {
            window.active_top_menu = Some(menu);
        }
    }
}

fn top_menu(
    ui: &mut egui::Ui,
    label: &str,
    underline: bool,
    force_open: bool,
    content: impl FnOnce(&mut egui::Ui),
) -> bool {
    let job = mnemonic_job(ui, label, underline);
    let (response, _) = egui::menu::MenuButton::from_button(egui::Button::new(job)).ui(ui, content);
    if force_open {
        let id = egui::Popup::menu(&response).get_id();
        egui::Popup::open_id(ui.ctx(), id);
        true
    } else {
        false
    }
}

fn menu_mnemonic(key: Key) -> Option<char> {
    match key {
        Key::F => Some('F'),
        Key::E => Some('E'),
        Key::V => Some('V'),
        Key::W => Some('W'),
        Key::H => Some('H'),
        _ => None,
    }
}

fn neighbor_top_menu(current: Option<char>, right: bool) -> Option<char> {
    const ORDER: [char; 5] = ['F', 'E', 'V', 'W', 'H'];
    let index = current.and_then(|menu| ORDER.iter().position(|&item| item == menu))?;
    let next = if right {
        (index + 1) % ORDER.len()
    } else {
        (index + ORDER.len() - 1) % ORDER.len()
    };
    Some(ORDER[next])
}

fn mnemonic_job(ui: &egui::Ui, label: &str, underline: bool) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut chars = label.chars();
    let Some(first) = chars.next() else {
        return job;
    };
    let rest: String = chars.collect();
    let color = ui.visuals().widgets.inactive.text_color();
    let mut first_fmt = egui::TextFormat {
        color,
        ..egui::TextFormat::default()
    };
    if underline {
        first_fmt.underline = Stroke::new(1.0, color);
    }
    job.append(&first.to_string(), 0.0, first_fmt);
    if !rest.is_empty() {
        job.append(
            &rest,
            0.0,
            egui::TextFormat {
                color,
                ..egui::TextFormat::default()
            },
        );
    }
    job
}

fn toolbar(ui: &mut egui::Ui, window: &mut Window) {
    ui.horizontal(|ui| {
        let new_file = tool_button(ui, ToolIcon::New);
        hover_note(&new_file, "New (Ctrl+N)");
        if new_file.clicked() {
            window.start_new_screen();
        }
        let open = tool_button(ui, ToolIcon::Open);
        hover_note(&open, "Open (Ctrl+O)");
        if open.clicked() {
            window.open_dialog();
        }
        let reopen = tool_button(ui, ToolIcon::Reopen);
        hover_note(&reopen, "Re-open (Ctrl+R)");
        if reopen.clicked() {
            window.reopen_current();
        }
        let recent = tool_button(ui, ToolIcon::Recent);
        hover_note(&recent, "Open Recent (Ctrl+Shift+R)");
        if recent.clicked() {
            window.recent_open = true;
        }
        let save = tool_button(ui, ToolIcon::Save);
        hover_note(&save, "Save (Ctrl+S)");
        if save.clicked() {
            window.save_current();
        }
        let save_as = tool_button(ui, ToolIcon::SaveAs);
        hover_note(&save_as, "Save As (Ctrl+Shift+S)");
        if save_as.clicked() {
            window.save_as_dialog();
        }
        let close = tool_button(ui, ToolIcon::Close);
        hover_note(&close, "Close (Ctrl+W)");
        if close.clicked() {
            window.end_file(ui.ctx());
        }
        ui.separator();
        let cut = tool_button(ui, ToolIcon::Cut);
        hover_note(&cut, "Cut (Ctrl+X)");
        if cut.clicked() {
            window.cut_selection(ui.ctx());
        }
        let copy = tool_button(ui, ToolIcon::Copy);
        hover_note(&copy, "Copy (Ctrl+C)");
        if copy.clicked() {
            window.copy_selection(ui.ctx());
        }
        let paste = tool_button(ui, ToolIcon::Paste);
        hover_note(&paste, "Paste (Ctrl+V)");
        if paste.clicked() {
            window.paste_from_clipboard();
        }
        ui.separator();
        let undo = tool_button(ui, ToolIcon::Undo);
        hover_note(&undo, "Undo (Ctrl+Z)");
        if undo.clicked() {
            window.view.handle_key(EditorKey::Undo);
        }
        let redo = tool_button(ui, ToolIcon::Redo);
        hover_note(&redo, "Redo (Ctrl+Y)");
        if redo.clicked() {
            window.view.handle_key(EditorKey::Redo);
        }
        ui.separator();
        let options = tool_button(ui, ToolIcon::Options);
        hover_note(&options, "Settings (Ctrl+Shift+O)");
        if options.clicked() {
            window.options_open = true;
        }
        ui.separator();
        let back = ui.add_enabled(window.can_jump_back(), tool_button_widget(ToolIcon::Back));
        hover_note(&back, "Back (Ctrl+Left)");
        if back.clicked() {
            window.jump_back();
        }
        let forward = ui.add_enabled(
            window.can_jump_forward(),
            tool_button_widget(ToolIcon::Forward),
        );
        hover_note(&forward, "Forward (Ctrl+Right)");
        if forward.clicked() {
            window.jump_forward();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let about = tool_button(ui, ToolIcon::About);
            hover_note(&about, "About");
            if about.clicked() {
                window.about_open = true;
            }
        });
    });
}

const TOOL_ICON: Color32 = Color32::from_rgb(130, 176, 255);

#[derive(Clone, Copy)]
enum ToolIcon {
    New,
    Open,
    Reopen,
    Recent,
    Save,
    SaveAs,
    Close,
    Cut,
    Copy,
    Paste,
    Undo,
    Redo,
    Options,
    Back,
    Forward,
    About,
}

fn tool_button(ui: &mut egui::Ui, icon: ToolIcon) -> egui::Response {
    ui.add(tool_button_widget(icon))
}

fn tool_button_widget(icon: ToolIcon) -> egui::Button<'static> {
    egui::Button::new(tool_icon_atom(icon))
}

fn tool_icon_atom(icon: ToolIcon) -> egui::Atom<'static> {
    egui::Atom::paint(Vec2::splat(22.0), move |ui, args| {
        paint_tool_icon(ui.painter(), args.rect, icon);
    })
}

fn paint_tool_icon(painter: &egui::Painter, rect: Rect, icon: ToolIcon) {
    let color = TOOL_ICON;
    match icon {
        ToolIcon::New => {
            icon_loop(
                painter,
                rect,
                &[
                    (3.5, 2.2),
                    (9.6, 2.2),
                    (12.5, 5.0),
                    (12.5, 13.8),
                    (3.5, 13.8),
                ],
                color,
            );
            icon_line(painter, rect, &[(9.6, 2.2), (9.6, 5.0), (12.5, 5.0)], color);
            icon_line(painter, rect, &[(8.0, 7.1), (8.0, 11.3)], color);
            icon_line(painter, rect, &[(5.9, 9.2), (10.1, 9.2)], color);
        }
        ToolIcon::Open => icon_loop(
            painter,
            rect,
            &[
                (2.2, 3.4),
                (6.8, 3.4),
                (8.1, 5.1),
                (13.8, 5.1),
                (13.8, 13.0),
                (2.2, 13.0),
            ],
            color,
        ),
        ToolIcon::Reopen => {
            icon_arc(painter, rect, 8.0, 8.2, 5.1, -1.05, 4.4, color);
            icon_line(
                painter,
                rect,
                &[(12.8, 2.2), (12.8, 5.3), (9.7, 5.3)],
                color,
            );
        }
        ToolIcon::Recent => {
            icon_circle(painter, rect, 8.0, 8.0, 5.3, color);
            icon_line(painter, rect, &[(8.0, 4.8), (8.0, 8.0), (10.3, 9.5)], color);
        }
        ToolIcon::Save => {
            icon_loop(
                painter,
                rect,
                &[
                    (3.0, 2.2),
                    (10.1, 2.2),
                    (13.0, 5.1),
                    (13.0, 13.8),
                    (3.0, 13.8),
                ],
                color,
            );
            icon_line(painter, rect, &[(5.0, 2.2), (5.0, 6.0), (10.0, 6.0)], color);
            icon_line(
                painter,
                rect,
                &[(5.0, 13.8), (5.0, 9.0), (11.0, 9.0), (11.0, 13.8)],
                color,
            );
        }
        ToolIcon::SaveAs => {
            icon_loop(
                painter,
                rect,
                &[
                    (2.2, 3.0),
                    (8.4, 3.0),
                    (10.6, 5.2),
                    (10.6, 12.0),
                    (2.2, 12.0),
                ],
                color,
            );
            icon_line(painter, rect, &[(4.0, 3.0), (4.0, 5.8), (8.0, 5.8)], color);
            icon_line(
                painter,
                rect,
                &[(4.0, 12.0), (4.0, 8.4), (8.2, 8.4), (8.2, 12.0)],
                color,
            );
            icon_line(painter, rect, &[(10.2, 8.6), (13.4, 11.8)], color);
            icon_line(painter, rect, &[(11.6, 8.2), (13.8, 10.4)], color);
        }
        ToolIcon::Close => {
            icon_loop(
                painter,
                rect,
                &[(3.2, 2.2), (12.8, 2.2), (12.8, 13.8), (3.2, 13.8)],
                color,
            );
            icon_line(painter, rect, &[(6.0, 6.0), (10.0, 10.0)], color);
            icon_line(painter, rect, &[(10.0, 6.0), (6.0, 10.0)], color);
        }
        ToolIcon::Undo => {
            icon_line(painter, rect, &[(6.2, 6.0), (3.2, 6.0), (3.2, 3.0)], color);
            icon_arc(painter, rect, 8.2, 8.2, 4.7, 3.55, 8.55, color);
        }
        ToolIcon::Redo => {
            icon_line(
                painter,
                rect,
                &[(9.8, 6.0), (12.8, 6.0), (12.8, 3.0)],
                color,
            );
            icon_arc(painter, rect, 7.8, 8.2, 4.7, -0.41, -5.41, color);
        }
        ToolIcon::Options => {
            icon_line(painter, rect, &[(2.2, 4.4), (4.8, 4.4)], color);
            icon_line(painter, rect, &[(8.0, 4.4), (13.8, 4.4)], color);
            icon_circle(painter, rect, 6.4, 4.4, 1.45, color);
            icon_line(painter, rect, &[(2.2, 8.0), (8.1, 8.0)], color);
            icon_line(painter, rect, &[(11.2, 8.0), (13.8, 8.0)], color);
            icon_circle(painter, rect, 9.6, 8.0, 1.45, color);
            icon_line(painter, rect, &[(2.2, 11.6), (6.1, 11.6)], color);
            icon_line(painter, rect, &[(9.2, 11.6), (13.8, 11.6)], color);
            icon_circle(painter, rect, 7.6, 11.6, 1.45, color);
        }
        ToolIcon::Back => icon_line(
            painter,
            rect,
            &[(10.2, 3.2), (5.4, 8.0), (10.2, 12.8)],
            color,
        ),
        ToolIcon::Forward => icon_line(
            painter,
            rect,
            &[(5.8, 3.2), (10.6, 8.0), (5.8, 12.8)],
            color,
        ),
        ToolIcon::Cut => {
            icon_circle(painter, rect, 5.4, 12.2, 2.0, color);
            icon_circle(painter, rect, 10.6, 12.2, 2.0, color);
            icon_line(painter, rect, &[(6.6, 10.6), (13.4, 2.2)], color);
            icon_line(painter, rect, &[(9.4, 10.6), (2.6, 2.2)], color);
        }
        ToolIcon::Copy => {
            icon_loop(
                painter,
                rect,
                &[(5.4, 2.0), (13.4, 2.0), (13.4, 11.0), (5.4, 11.0)],
                color,
            );
            icon_loop(
                painter,
                rect,
                &[(2.6, 4.8), (10.6, 4.8), (10.6, 13.8), (2.6, 13.8)],
                color,
            );
        }
        ToolIcon::Paste => {
            icon_loop(
                painter,
                rect,
                &[(4.2, 3.8), (11.8, 3.8), (11.8, 14.0), (4.2, 14.0)],
                color,
            );
            icon_loop(
                painter,
                rect,
                &[(6.2, 2.0), (9.8, 2.0), (9.8, 5.0), (6.2, 5.0)],
                color,
            );
            icon_line(painter, rect, &[(5.6, 7.4), (10.4, 7.4)], color);
            icon_line(painter, rect, &[(5.6, 9.6), (10.4, 9.6)], color);
            icon_line(painter, rect, &[(5.6, 11.8), (8.6, 11.8)], color);
        }
        ToolIcon::About => {
            icon_circle(painter, rect, 8.0, 8.0, 5.3, color);
            icon_line(painter, rect, &[(8.0, 7.2), (8.0, 11.2)], color);
            painter.circle_filled(icon_at(rect, 8.0, 5.0), icon_scale(rect) * 0.7, color);
        }
    }
}

fn icon_scale(rect: Rect) -> f32 {
    rect.width() / 16.0
}

fn icon_at(rect: Rect, x: f32, y: f32) -> Pos2 {
    let scale = icon_scale(rect);
    Pos2::new(rect.min.x + x * scale, rect.min.y + y * scale)
}

fn icon_stroke(rect: Rect, color: Color32) -> Stroke {
    Stroke::new(1.4 * icon_scale(rect), color)
}

fn icon_points(rect: Rect, points: &[(f32, f32)]) -> Vec<Pos2> {
    points.iter().map(|(x, y)| icon_at(rect, *x, *y)).collect()
}

fn icon_line(painter: &egui::Painter, rect: Rect, points: &[(f32, f32)], color: Color32) {
    painter.line(icon_points(rect, points), icon_stroke(rect, color));
}

fn icon_loop(painter: &egui::Painter, rect: Rect, points: &[(f32, f32)], color: Color32) {
    painter.add(egui::Shape::closed_line(
        icon_points(rect, points),
        icon_stroke(rect, color),
    ));
}

fn icon_circle(painter: &egui::Painter, rect: Rect, x: f32, y: f32, radius: f32, color: Color32) {
    painter.circle_stroke(
        icon_at(rect, x, y),
        radius * icon_scale(rect),
        icon_stroke(rect, color),
    );
}

fn icon_arc(
    painter: &egui::Painter,
    rect: Rect,
    cx: f32,
    cy: f32,
    radius: f32,
    start: f32,
    end: f32,
    color: Color32,
) {
    let steps = 18;
    let points: Vec<Pos2> = (0..=steps)
        .map(|step| {
            let t = start + (end - start) * (step as f32 / steps as f32);
            icon_at(rect, cx + radius * t.cos(), cy + radius * t.sin())
        })
        .collect();
    painter.line(points, icon_stroke(rect, color));
}

fn tab_strip(ui: &mut egui::Ui, window: &mut Window) {
    let current_id = window.view.session.current().id;
    let labels: Vec<(u32, String, bool)> = window
        .view
        .session
        .tabs()
        .iter()
        .map(|tab| {
            (
                tab.id,
                format!(
                    "{}{}",
                    tab.title,
                    if tab.doc.is_modified() { "*" } else { "" }
                ),
                tab.id == current_id,
            )
        })
        .collect();
    let mut select_id = None;
    let mut menu_at = None;
    let mut hover = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(8.0, 1.0);
        for (index, (id, label, selected)) in labels.iter().enumerate() {
            let response = ui
                .add(
                    egui::Button::selectable(*selected, label.as_str())
                        .sense(Sense::CLICK | Sense::DRAG),
                )
                .on_hover_and_drag_cursor(egui::CursorIcon::Default);
            if response.secondary_clicked() {
                let pos = ui
                    .input(|input| input.pointer.interact_pos())
                    .unwrap_or(response.rect.min);
                menu_at = Some(TabMenu { id: *id, pos });
            } else if response.clicked() {
                select_id = Some(*id);
            }
            if response.drag_started_by(egui::PointerButton::Primary) {
                window.tab_drag = Some(*id);
                window.tab_menu = None;
            }
            if response.contains_pointer() {
                hover = Some(index);
            }
        }
        let spare_w = ui.available_width();
        let row_h = ui.min_rect().height().max(ui.spacing().interact_size.y);
        if spare_w > 1.0 {
            let spare = ui.allocate_response(Vec2::new(spare_w, row_h), Sense::click());
            if spare.double_clicked() && window.tab_menu.is_none() {
                window.start_new_screen();
            }
        }
    });
    if let Some(menu) = menu_at {
        window.tab_menu = Some(menu);
        window.tab_menu_fresh = true;
        window.editor_menu = None;
    }
    if let Some(id) = select_id {
        let _ = window.select_tab(id);
    }
    if window.tab_drag.is_some() && ui.input(|input| !input.pointer.primary_down()) {
        window.tab_drag = None;
    } else if let (Some(drag_id), Some(to)) = (window.tab_drag, hover) {
        if let Some(from) = labels.iter().position(|(id, _, _)| *id == drag_id) {
            window.view.session.move_tab(from, to);
        }
    }
    show_tab_menu(ui, window);
}

/// Right-click menu for a variable, paragraph, or copybook. Aligned under the name.
#[derive(Clone)]
struct EditorMenu {
    pos: Pos2,
    line: usize,
    col: usize,
    path: Option<PathBuf>,
}

fn show_editor_menu(ui: &mut egui::Ui, window: &mut Window) {
    let Some(menu) = window.editor_menu.clone() else {
        return;
    };
    let pos = menu.pos;
    let target = ParagraphCall {
        col: 1,
        len: 1,
        line: menu.line,
        target_col: menu.col,
        path: menu.path.clone(),
    };
    let fresh = window.editor_menu_fresh;
    let mut chosen = false;
    let mut menu_rect = None;
    egui::Area::new(egui::Id::unique("editor_def_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            let inner = egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_width(180.0);
                    if ui
                        .add(
                            egui::Button::new("Go to definition")
                                .sense(Sense::CLICK)
                                .min_size(egui::vec2(180.0, 22.0)),
                        )
                        .clicked()
                    {
                        chosen = true;
                    }
                });
            });
            menu_rect = Some(inner.response.rect);
        });
    let pressed_outside = !fresh
        && ui.input(|input| input.pointer.any_pressed())
        && ui
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|pointer| menu_rect.is_none_or(|rect| !rect.contains(pointer)));
    window.editor_menu_fresh = false;
    if chosen || pressed_outside {
        window.editor_menu = None;
    }
    if chosen {
        window.follow_call(&target);
    }
}

/// Right-click menu for a file tab. Its top-left corner is the pointer.
#[derive(Clone, Copy)]
struct TabMenu {
    id: u32,
    pos: Pos2,
}

fn show_tab_menu(ui: &mut egui::Ui, window: &mut Window) {
    let Some(menu) = window.tab_menu else {
        return;
    };
    if !window
        .view
        .session
        .tabs()
        .iter()
        .any(|tab| tab.id == menu.id)
    {
        window.tab_menu = None;
        return;
    }
    let id = menu.id;
    let pos = menu.pos;
    let fresh = window.tab_menu_fresh;
    let mut action = None;
    let mut menu_rect = None;
    egui::Area::new(egui::Id::unique("file_tab_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            let inner = egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(180.0);
                ui.vertical(|ui| {
                    for (label, kind) in [
                        ("Close", TabAction::Close),
                        ("Save", TabAction::Save),
                        ("Save As", TabAction::SaveAs),
                        ("Move to new instance", TabAction::Move),
                    ] {
                        if ui
                            .add_sized([180.0, 22.0], egui::Button::new(label).sense(Sense::CLICK))
                            .clicked()
                        {
                            action = Some(kind);
                        }
                    }
                });
            });
            menu_rect = Some(inner.response.rect);
        });
    let pressed_outside = !fresh
        && ui.input(|input| input.pointer.any_pressed())
        && ui
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|pointer| menu_rect.is_none_or(|rect| !rect.contains(pointer)));
    window.tab_menu_fresh = false;
    if action.is_some() || pressed_outside {
        window.tab_menu = None;
    }
    if let Some(action) = action {
        if window.select_tab(id) {
            match action {
                TabAction::Close => window.end_file(ui.ctx()),
                TabAction::Save => window.save_current(),
                TabAction::SaveAs => window.save_as_dialog(),
                TabAction::Move => window.move_tab_to_new_instance(ui.ctx()),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum TabAction {
    Close,
    Save,
    SaveAs,
    Move,
}

fn spawn_instance(path: &std::path::Path, view: &mut EditorView) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        view.set_status("Cannot start a new instance");
        return false;
    };
    match std::process::Command::new(exe)
        .arg("--center")
        .arg(path)
        .spawn()
    {
        Ok(_) => true,
        Err(err) => {
            view.set_status(format!("Cannot start a new instance: {err}"));
            false
        }
    }
}

/// One stop in the Ctrl-click history, including the scroll position at that stop.
#[derive(Clone)]
struct JumpPlace {
    tab: u32,
    line: usize,
    col: usize,
    top: usize,
    left: usize,
}

/// Vertical scroll that brings `idx` into the window, leaving it where it is when it is already shown.
fn push_recent(recent: &mut Vec<std::path::PathBuf>, path: std::path::PathBuf, limit: usize) {
    recent.retain(|item| !same_file(item, &path));
    recent.insert(0, path);
    recent.truncate(limit.max(1));
}

fn drop_open_paths(recent: &mut Vec<std::path::PathBuf>, open: &[std::path::PathBuf]) {
    recent.retain(|item| !open.iter().any(|path| same_file(item, path)));
}

/// Ctrl-click puts the target on the second editor line, with one line above it.
fn top_for_second_line(index: usize) -> usize {
    index.saturating_sub(1)
}

fn revealed_top(top: usize, idx: usize, page: usize) -> usize {
    let page = page.max(1);
    if idx < top {
        idx
    } else if idx >= top + page {
        idx + 1 - page
    } else {
        top
    }
}

/// Horizontal scroll that brings column `col0` into view.
fn revealed_left(left: usize, col0: usize, cols: usize) -> usize {
    let cols = cols.max(1);
    if col0 < left {
        col0
    } else if col0 >= left + cols {
        col0 + 1 - cols
    } else {
        left
    }
}

/// The saved top still shows this visible-row index.
fn scroll_can_show(top: usize, line_at: Option<usize>, page: usize, row_count: usize) -> bool {
    let Some(idx) = line_at else {
        return false;
    };
    if row_count == 0 {
        return false;
    }
    let page = page.max(1);
    top <= row_count - 1 && idx >= top && idx < top + page
}

fn move_swap_list(at: usize, last: usize, down: bool) -> usize {
    if down {
        (at + 1).min(last)
    } else {
        at.saturating_sub(1)
    }
}

fn hold_dialog_focus(response: &egui::Response) {
    response.request_focus();
    response.ctx.memory_mut(|mem| {
        mem.move_focus(egui::FocusDirection::None);
        mem.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });
}

fn status_message(view: &EditorView) -> &str {
    if view.status().is_empty() {
        "Ready"
    } else {
        view.status()
    }
}

fn fit_right(text: &str, cols: usize) -> String {
    let count = text.chars().count();
    if count <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    if cols == 1 {
        return "…".to_string();
    }
    let mut fitted: String = text.chars().take(cols - 1).collect();
    fitted.push('…');
    fitted
}

fn next_line_label(at: NextLineCursor) -> &'static str {
    match at {
        NextLineCursor::LineNumber => "Line number",
        NextLineCursor::Text => "Text",
    }
}

fn fit_line(text: &str, cols: usize) -> String {
    let count = text.chars().count();
    if count <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    if cols == 1 {
        return "…".to_string();
    }
    let mut fitted: String = text.chars().take(cols - 1).collect();
    fitted.push('…');
    fitted
}

fn status_bar_height(row_h: f32) -> f32 {
    row_h + 2.0
}

fn status_line(ui: &mut egui::Ui, view: &EditorView, font: &FontId, char_w: f32, row_h: f32) {
    let (line, data_col) = view.cursor();
    let col = if view.focus() == Focus::Command {
        view.command_col()
    } else {
        data_col
    };
    let doc = &view.session.current().doc;
    let seq = doc.line(line).map(|row| row.seq).unwrap_or(0);
    let mode = if view.insert_mode() { "INS" } else { "OVR" };
    let hex = if doc.hex_mode() { "    HEX" } else { "" };
    let cols = if doc.cols_mode() { "    COLS" } else { "" };
    let text = format!(
        "{mode}    line {seq}  col {col}    {}    {} lines{hex}{cols}",
        doc.encoding().name(),
        doc.last_real()
    );
    let bar_h = status_bar_height(row_h);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), bar_h), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_rgb(240, 240, 240));
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, Color32::from_rgb(170, 170, 180)),
    );
    let pad_x = 8.0;
    let inner_w = (rect.width() - pad_x * 2.0).max(0.0);
    let cols = (inner_w / char_w).floor().max(0.0) as usize;
    let text = fit_line(&text, cols);
    let origin_y = rect.center().y - row_h / 2.0;
    let painter = ui.painter().with_clip_rect(rect);
    let mut x = rect.left() + pad_x;
    for ch in text.chars() {
        painter.text(
            Pos2::new(x, origin_y),
            Align2::LEFT_TOP,
            ch.to_string(),
            font.clone(),
            Color32::from_rgb(20, 20, 32),
        );
        x += char_w;
    }
}

fn window_title(view: &EditorView) -> String {
    let tab = view.session.current();
    let star = if tab.doc.is_modified() { "*" } else { "" };
    let name = tab
        .path
        .as_ref()
        .map(PathBuf::as_path)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| tab.title.clone());
    format!("ISPFLite v{VERSION} — {name}{star}")
}

fn scrollbar_thumb(bar: Rect, top: usize, max_top: usize, page: usize, row_count: usize) -> Rect {
    let total = row_count.max(1) as f32;
    let thumb_h = (bar.height() * page as f32 / total).clamp(18.0, bar.height().max(18.0));
    let travel = (bar.height() - thumb_h).max(0.0);
    let along = if max_top == 0 {
        0.0
    } else {
        top as f32 / max_top as f32
    };
    Rect::from_min_size(
        Pos2::new(bar.left() + 2.0, bar.top() + along * travel),
        Vec2::new((bar.width() - 4.0).max(4.0), thumb_h),
    )
}

fn paint_string(
    ui: &egui::Ui,
    origin: Pos2,
    text: &str,
    font: &FontId,
    color: Color32,
    char_w: f32,
) {
    let mut x = origin.x;
    for ch in text.chars() {
        paint_glyph(ui, Pos2::new(x, origin.y), ch, font, color);
        x += char_w;
    }
}

fn column_scale(first_col: usize, count: usize) -> String {
    let mut cells = vec!['-'; count];
    for index in 0..count {
        let col = first_col + index;
        if col % 10 == 0 {
            for (offset, ch) in (col / 10).to_string().chars().enumerate() {
                if index + offset < count {
                    cells[index + offset] = ch;
                }
            }
        } else if col % 5 == 0 && cells[index] == '-' {
            cells[index] = '+';
        }
    }
    cells.into_iter().collect()
}

const FIND_TEXT: Color32 = Color32::from_rgb(0, 0, 32);

fn paint_find_marks(
    ui: &egui::Ui,
    origin: Pos2,
    spans: &[(usize, usize)],
    chars: &[char],
    font: &FontId,
    hscroll: usize,
    char_w: f32,
    row_h: f32,
    limit: usize,
) {
    for (col, len) in spans {
        paint_span(
            ui,
            origin,
            *col,
            *len,
            hscroll,
            char_w,
            row_h,
            limit,
            Color32::WHITE,
        );
        let start = col.saturating_sub(1);
        let end = start + len;
        for (offset, ch) in chars.iter().enumerate() {
            let absolute = hscroll + offset;
            if absolute >= start && absolute < end && offset < limit {
                paint_glyph(
                    ui,
                    Pos2::new(origin.x + offset as f32 * char_w, origin.y),
                    *ch,
                    font,
                    FIND_TEXT,
                );
            }
        }
    }
}

fn paint_span(
    ui: &egui::Ui,
    origin: Pos2,
    col: usize,
    len: usize,
    hscroll: usize,
    char_w: f32,
    row_h: f32,
    limit: usize,
    color: Color32,
) {
    if len == 0 {
        return;
    }
    let start = col.saturating_sub(1);
    let end = start + len;
    if end <= hscroll || start >= hscroll + limit {
        return;
    }
    let vis_start = start.max(hscroll) - hscroll;
    let vis_end = (end - hscroll).min(limit);
    let rect = Rect::from_min_size(
        Pos2::new(origin.x + vis_start as f32 * char_w, origin.y),
        Vec2::new((vis_end - vis_start) as f32 * char_w, row_h),
    );
    ui.painter().rect_filled(rect, 0.0, color);
}

fn sentinel_line(label: &str) -> String {
    const WIDTH: usize = 80;
    let len = label.chars().count();
    if len >= WIDTH {
        return label.chars().take(WIDTH).collect();
    }
    let left = (WIDTH - len) / 2;
    let right = WIDTH - len - left;
    format!("{}{label}{}", "*".repeat(left), "*".repeat(right))
}

fn exclude_summary(count: i32) -> String {
    const WIDTH: usize = 80;
    let label = format!("{} Line(s) not Displayed", count.max(0));
    let len = label.chars().count();
    if len >= WIDTH {
        return label.chars().take(WIDTH).collect();
    }
    let left = (WIDTH - len) / 2;
    let right = WIDTH - len - left;
    format!("{}{label}{}", dash_fill(left), dash_fill(right))
}

fn dash_fill(columns: usize) -> String {
    let mut out = String::new();
    while out.chars().count() + 2 <= columns {
        out.push_str("- ");
    }
    if out.chars().count() < columns {
        out.push('-');
    }
    out
}

fn paint_chars(
    ui: &egui::Ui,
    origin: Pos2,
    chars: &[char],
    kinds: &[u8],
    font: &FontId,
    color: Color32,
    colors: &SyntaxColors,
    char_w: f32,
    limit: usize,
) {
    let mut x = origin.x;
    for (index, ch) in chars.iter().take(limit).enumerate() {
        let paint = kinds
            .get(index)
            .copied()
            .map(|kind| kind_color(kind, color, colors))
            .unwrap_or(color);
        paint_glyph(ui, Pos2::new(x, origin.y), *ch, font, paint);
        x += char_w;
    }
}

fn kind_color(kind: u8, text: Color32, colors: &SyntaxColors) -> Color32 {
    match kind {
        KEYWORD => colors.keyword,
        COBOL_KEYWORD => colors.cobol_keyword,
        COMMENT => colors.comment,
        STRING => colors.string,
        NUMBER => colors.number,
        DIRECTIVE => colors.directive,
        _ => text,
    }
}

fn paint_call_underline(
    ui: &egui::Ui,
    origin: Pos2,
    link: &ParagraphCall,
    hscroll: usize,
    char_w: f32,
    row_h: f32,
    limit: usize,
    color: Color32,
) {
    let start = link.col.saturating_sub(1);
    let end = start + link.len;
    if end <= hscroll || start >= hscroll + limit {
        return;
    }
    let vis_start = start.max(hscroll) - hscroll;
    let vis_end = (end - hscroll).min(limit);
    let y = origin.y + row_h - 2.0;
    ui.painter().hline(
        (origin.x + vis_start as f32 * char_w)..=(origin.x + vis_end as f32 * char_w),
        y,
        Stroke::new(1.0, color),
    );
}

fn paint_cursor(
    ui: &egui::Ui,
    origin: Pos2,
    ch: char,
    font: &FontId,
    char_w: f32,
    row_h: f32,
    insert: bool,
    color: Color32,
) {
    if insert {
        let rect = Rect::from_min_size(
            Pos2::new(origin.x, origin.y + row_h / 2.0),
            Vec2::new(char_w, row_h / 2.0),
        );
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(170, 210, 255));
    } else {
        let y = origin.y + row_h - 2.0;
        ui.painter().hline(
            origin.x..=(origin.x + char_w),
            y,
            Stroke::new(2.0, Color32::WHITE),
        );
    }
    paint_glyph(ui, origin, ch, font, color);
}

fn paint_glyph(ui: &egui::Ui, origin: Pos2, ch: char, font: &FontId, color: Color32) {
    ui.painter().text(
        origin,
        Align2::LEFT_TOP,
        ch.to_string(),
        font.clone(),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_setting_picks_the_attention_key() {
        assert!(is_attention(EnterKey::Enter, Key::Enter, None));
        assert!(!is_attention(
            EnterKey::Enter,
            Key::ControlRight,
            Some(Key::ControlRight)
        ));

        assert!(is_attention(
            EnterKey::RightCtrl,
            Key::ControlRight,
            Some(Key::ControlRight)
        ));
        assert!(!is_attention(
            EnterKey::RightCtrl,
            Key::Enter,
            Some(Key::Enter)
        ));
        assert!(!is_attention(
            EnterKey::RightCtrl,
            Key::ControlLeft,
            Some(Key::ControlLeft)
        ));

        assert!(is_attention(
            EnterKey::LeftCtrl,
            Key::ControlLeft,
            Some(Key::ControlLeft)
        ));
        assert!(is_attention(
            EnterKey::BothCtrl,
            Key::ControlLeft,
            Some(Key::ControlLeft)
        ));
        assert!(is_attention(
            EnterKey::BothCtrl,
            Key::ControlRight,
            Some(Key::ControlRight)
        ));
    }

    #[test]
    fn a_ctrl_used_as_enter_is_not_a_shortcut_modifier() {
        assert!(ctrl_modifies(EnterKey::Enter, false, false, true));
        assert!(ctrl_modifies(EnterKey::RightCtrl, true, false, true));
        assert!(!ctrl_modifies(EnterKey::RightCtrl, false, true, true));
        assert!(ctrl_modifies(EnterKey::LeftCtrl, false, true, true));
        assert!(!ctrl_modifies(EnterKey::LeftCtrl, true, false, true));
        assert!(!ctrl_modifies(EnterKey::BothCtrl, true, true, true));
        assert!(is_attention_ctrl(
            EnterKey::BothCtrl,
            Key::ControlLeft,
            Some(Key::ControlLeft)
        ));
        assert!(!is_attention_ctrl(
            EnterKey::Enter,
            Key::ControlLeft,
            Some(Key::ControlLeft)
        ));
        assert!(release_runs_enter(true, false, false));
        assert!(!release_runs_enter(true, true, false));
        assert!(!release_runs_enter(true, false, true));
    }

    #[test]
    fn pcom_theme_uses_the_traditional_3270_colors() {
        let current = syntax_colors(ColorTheme::Current);
        let pcom = syntax_colors(ColorTheme::Pcom);
        assert_eq!(
            kind_color(KEYWORD, current.text, &current),
            Color32::from_rgb(130, 176, 255)
        );
        assert_eq!(pcom.text, Color32::from_rgb(0, 255, 0));
        assert_eq!(pcom.background, Color32::from_rgb(0, 0, 0));
        assert_eq!(
            kind_color(KEYWORD, pcom.text, &pcom),
            Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(
            kind_color(COMMENT, pcom.text, &pcom),
            Color32::from_rgb(0, 255, 255)
        );
        assert_eq!(
            kind_color(STRING, pcom.text, &pcom),
            Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(
            kind_color(NUMBER, pcom.text, &pcom),
            Color32::from_rgb(0, 255, 0)
        );
        assert_eq!(pcom.sentinel, LINE_NUMBER_COLOR);
        assert_eq!(
            kind_color(COBOL_KEYWORD, pcom.text, &pcom),
            Color32::from_rgb(255, 0, 0)
        );
        assert_eq!(kind_color(DIRECTIVE, pcom.text, &pcom), LINE_NUMBER_COLOR);
        assert_eq!(ColorTheme::CHOICES[0].label(), "Classic");
        assert_eq!(ColorTheme::CHOICES[1].label(), "Dark Blue");

        let top = sentinel_line("Top of Data");
        let bottom = sentinel_line("Bottom of Data");
        assert_eq!(top.chars().count(), 80);
        assert_eq!(bottom.chars().count(), 80);
        let top_at = top.find("Top of Data").unwrap();
        let bottom_at = bottom.find("Bottom of Data").unwrap();
        assert!(top[..top_at].chars().all(|ch| ch == '*'));
        assert!(top[top_at + "Top of Data".len()..]
            .chars()
            .all(|ch| ch == '*'));
        assert!(bottom[..bottom_at].chars().all(|ch| ch == '*'));
        assert!(bottom[bottom_at + "Bottom of Data".len()..]
            .chars()
            .all(|ch| ch == '*'));
        assert!(
            (top[..top_at].len() as i32 - top[top_at + "Top of Data".len()..].len() as i32).abs()
                <= 1
        );
    }

    #[test]
    fn exclude_summary_is_centered_between_dashes() {
        let line = exclude_summary(3);
        let label = "3 Line(s) not Displayed";
        assert_eq!(line.chars().count(), 80);
        let at = line.find(label).unwrap();
        let left = &line[..at];
        let right = &line[at + label.len()..];
        assert!(left.chars().all(|ch| ch == '-' || ch == ' '));
        assert!(right.chars().all(|ch| ch == '-' || ch == ' '));
        assert!((left.chars().count() as i32 - right.chars().count() as i32).abs() <= 1);
        assert!(left.contains("- "));
        assert!(right.contains("- "));
    }

    #[test]
    fn jump_scroll_is_restored_when_the_line_is_still_in_that_window() {
        let one = std::path::PathBuf::from("C:\\docs\\one.txt");
        let two = std::path::PathBuf::from("C:\\docs\\two.txt");
        let mut recent = vec![one.clone(), two.clone()];
        drop_open_paths(&mut recent, &[one.clone()]);
        assert_eq!(recent, vec![two.clone()]);
        push_recent(&mut recent, std::path::PathBuf::from("c:/docs/one.txt"), 10);
        assert!(same_file(&recent[0], &one));
        assert_eq!(recent.len(), 2);
        assert_eq!(top_for_second_line(0), 0);
        assert_eq!(top_for_second_line(1), 0);
        assert_eq!(top_for_second_line(8), 7);
        assert_eq!(revealed_top(40, 12, 24), 12);
        assert_eq!(revealed_top(10, 12, 24), 10);
        assert_eq!(revealed_top(0, 30, 24), 7);
        assert_eq!(revealed_left(0, 40, 10), 31);
        assert_eq!(revealed_left(20, 25, 10), 20);
        assert!(scroll_can_show(80, Some(90), 24, 200));
        assert!(!scroll_can_show(80, Some(10), 24, 200));
        assert!(!scroll_can_show(190, Some(10), 24, 50));
    }

    #[test]
    fn swap_list_arrows_stay_on_a_row() {
        assert_eq!(move_swap_list(0, 3, false), 0);
        assert_eq!(move_swap_list(0, 3, true), 1);
        assert_eq!(move_swap_list(3, 3, true), 3);
        assert_eq!(move_swap_list(2, 3, false), 1);
    }
}
