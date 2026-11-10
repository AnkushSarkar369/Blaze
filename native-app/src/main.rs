use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use blaze_engine::{MediaEngine, MediaInfo};
use blaze_engine::export::{self, InterchangeFormat};
use blaze_engine::silence::{
    DetectionLevel, PaddingPreset, SilenceDetectionConfig, SilenceInterval,
};
// Silence detection UI integration follows.
use eframe::egui;
use playback::Playback;
use rfd::FileDialog;

mod playback;
mod waveform;

use waveform::Waveform;

const APP_TITLE: &str = "Blaze — Ankush Sarkar";
const FREE_EXPORT_LIMIT: u32 = 2;
const GUMROAD_VERIFY_URL: &str = "https://api.gumroad.com/v2/licenses/verify";
const GUMROAD_PRODUCTS: [(&str, LicensePlan); 2] = [
    ("Niyme89XCcFC44ShhEPPrQ==", LicensePlan::Monthly),
    ("ksPYCeAJW8TtmOjsBRQ0OA==", LicensePlan::Permanent),
];
const DETECTION_DB_VALUES: [f64; 5] = [32.0, 26.0, 22.0, 18.0, 14.0];

const COLOR_BG: egui::Color32 = egui::Color32::from_rgb(10, 11, 12);
const COLOR_SURFACE: egui::Color32 = egui::Color32::from_rgb(19, 21, 24);
const COLOR_SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(30, 32, 35);
const COLOR_BORDER: egui::Color32 = egui::Color32::from_rgb(43, 46, 50);
const COLOR_TEXT: egui::Color32 = egui::Color32::from_rgb(239, 241, 243);
const COLOR_MUTED: egui::Color32 = egui::Color32::from_rgb(177, 182, 188);
const COLOR_ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 104, 79);
const COLOR_ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(255, 143, 119);

fn apply_blaze_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = COLOR_BG;
    visuals.window_fill = COLOR_BG;
    visuals.extreme_bg_color = COLOR_BG;
    visuals.faint_bg_color = COLOR_BG;
    visuals.widgets.noninteractive.bg_fill = COLOR_BG;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, COLOR_TEXT);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, COLOR_BORDER);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(34, 36, 39);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, COLOR_BORDER);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, COLOR_TEXT);
    visuals.widgets.hovered.bg_fill = COLOR_SURFACE_HOVER;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, COLOR_ACCENT_HOVER);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, COLOR_TEXT);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(39, 34, 33);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, COLOR_ACCENT);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, COLOR_TEXT);
    visuals.selection.bg_fill = egui::Color32::from_rgb(92, 45, 39);
    visuals.selection.stroke = egui::Stroke::new(1.0, COLOR_ACCENT);
    visuals.hyperlink_color = COLOR_ACCENT_HOVER;
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style.animation_time = 0.14;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(14.0, 9.0);
        style.spacing.interact_size = egui::vec2(40.0, 28.0);
        style.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace));
        style.visuals.override_text_color = Some(COLOR_TEXT);
    });
}

fn open_external_url(url: &str) -> Result<(), String> {
    open::that(url).map_err(|error| format!("Could not open the browser: {error}"))
}

fn request_gumroad_purchase(
    client: &reqwest::blocking::Client,
    license_key: &str,
    product_id: &str,
    increment_uses: bool,
) -> Result<Option<serde_json::Value>, String> {
    let response = client
        .post(GUMROAD_VERIFY_URL)
        .form(&[
            ("product_id", product_id),
            ("license_key", license_key),
            ("increment_uses_count", if increment_uses { "true" } else { "false" }),
        ])
        .send()
        .map_err(|error| {
            format!(
                "Could not reach Gumroad. Check your internet connection and try again. ({error})"
            )
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // Gumroad returns 404 when a key does not belong to the supplied product.
        return Ok(None);
    }

    let body: serde_json::Value = response
        .json()
        .map_err(|error| format!("Gumroad returned an unreadable response: {error}"))?;

    if !status.is_success() {
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("Gumroad could not verify this licence.");
        return Err(format!("Gumroad verification failed (HTTP {status}): {message}"));
    }

    if body.get("success").and_then(|value| value.as_bool()) != Some(true) {
        return Ok(None);
    }

    body.get("purchase")
        .cloned()
        .map(Some)
        .ok_or_else(|| "Gumroad did not return purchase details.".to_owned())
}

fn parse_gumroad_timestamp(value: &str) -> Option<std::time::SystemTime> {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }

    let number = |start: usize, end: usize| -> Option<i64> {
        let slice = bytes.get(start..end)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };

    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;

    if year < 1
        || !(1..=12).contains(&month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }

    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day < 1 || day > days_in_month {
        return None;
    }

    // Convert the Gregorian calendar date to days since 1970-01-01.
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;

    let mut position = 19;
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let fraction_start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == fraction_start {
            return None;
        }
    }

    let offset_seconds = match bytes.get(position)? {
        b'Z' => {
            position += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            if bytes.get(position + 3) != Some(&b':') {
                return None;
            }
            let offset_hours = number(position + 1, position + 3)?;
            let offset_minutes = number(position + 4, position + 6)?;
            if offset_hours > 23 || offset_minutes > 59 {
                return None;
            }
            position += 6;
            let offset = offset_hours * 3_600 + offset_minutes * 60;
            if *sign == b'+' { offset } else { -offset }
        }
        _ => return None,
    };
    if position != bytes.len() {
        return None;
    }

    let unix_seconds = days_since_epoch
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?
        .checked_sub(offset_seconds)?;
    if unix_seconds < 0 {
        return None;
    }

    std::time::UNIX_EPOCH.checked_add(Duration::from_secs(unix_seconds as u64))
}

fn evaluate_gumroad_purchase(
    purchase: &serde_json::Value,
    product_id: &str,
    plan: LicensePlan,
) -> LicenseResult {
    if purchase.get("product_id").and_then(|value| value.as_str()) != Some(product_id) {
        return Err("Gumroad returned a purchase for a different product.".to_owned());
    }
    if purchase.get("refunded").and_then(|value| value.as_bool()) == Some(true) {
        return Err("This purchase has been refunded.".to_owned());
    }
    if purchase.get("chargebacked").and_then(|value| value.as_bool()) == Some(true) {
        return Err("This purchase has been charged back.".to_owned());
    }
    if purchase.get("disputed").and_then(|value| value.as_bool()) == Some(true)
        && purchase.get("dispute_won").and_then(|value| value.as_bool()) != Some(true)
    {
        return Err("This purchase is currently disputed.".to_owned());
    }

    let valid_until = if plan == LicensePlan::Monthly {
        if purchase.get("recurrence").and_then(|value| value.as_str()) != Some("monthly") {
            return Err("This key is not linked to a Blaze Monthly subscription.".to_owned());
        }

        let mut next_end: Option<(std::time::SystemTime, String)> = None;
        for field in [
            "subscription_cancelled_at",
            "subscription_ended_at",
            "subscription_failed_at",
        ] {
            let Some(value) = purchase.get(field) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let timestamp = value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("Gumroad returned an invalid {field} timestamp."))?;
            let end = parse_gumroad_timestamp(timestamp)
                .ok_or_else(|| format!("Gumroad returned an unreadable {field} timestamp."))?;

            if end <= std::time::SystemTime::now() {
                return Err("Your Blaze Monthly subscription period has ended.".to_owned());
            }

            let earlier_end = next_end
                .as_ref()
                .map_or(true, |(current_end, _)| end < *current_end);
            if earlier_end {
                next_end = Some((end, timestamp.to_owned()));
            }
        }
        next_end.map(|(_, timestamp)| timestamp)
    } else {
        None
    };

    Ok(LicenseEntitlement { plan, valid_until })
}

fn verify_gumroad_license(license_key: &str, increment_uses: bool) -> LicenseResult {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not prepare license verification: {error}"))?;

    // Monthly is checked first, followed by Permanent.
    for (product_id, plan) in GUMROAD_PRODUCTS {
        let Some(purchase) = request_gumroad_purchase(&client, license_key, product_id, false)? else {
            continue;
        };

        // A matched but revoked or expired purchase must not fall through.
        let entitlement = evaluate_gumroad_purchase(&purchase, product_id, plan)?;
        if !increment_uses {
            return Ok(entitlement);
        }

        // Increment Uses only after a successful, uncounted entitlement check.
        let counted_purchase = request_gumroad_purchase(&client, license_key, product_id, true)?
            .ok_or_else(|| "Gumroad could not confirm this licence activation.".to_owned())?;
        return evaluate_gumroad_purchase(&counted_purchase, product_id, plan);
    }

    Err("This key does not match an active Blaze Monthly or Permanent licence.".to_owned())
}


fn license_state_file() -> Option<PathBuf> {
    let config_dir = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }?;

    Some(config_dir.join("Blaze").join("license-state"))
}

fn load_license_state() -> (u32, Option<String>) {
    let Some(path) = license_state_file() else {
        return (0, None);
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return (0, None);
    };

    let mut lines = contents.lines();
    let _stored_activation = lines.next() == Some("true");
    let successful_exports = lines
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
        .min(FREE_EXPORT_LIMIT);
    let third_line = lines.next();
    let (license_key, marker) = if third_line == Some("gumroad-v1") {
        (None, true)
    } else {
        (third_line.filter(|value| !value.is_empty()).map(str::to_owned), lines.next() == Some("gumroad-v1"))
    };

    if marker {
        (successful_exports, license_key)
    } else {
        (successful_exports, None)
    }
}

fn persist_license_state(activated: bool, successful_exports: u32, license_key: Option<&str>) {
    let Some(path) = license_state_file() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        eprintln!("Blaze: could not create license state directory: {error}");
        return;
    }
    let key = license_key.unwrap_or("");
    if let Err(error) = std::fs::write(
        path,
        format!("{}\n{}\n{}\ngumroad-v1\n", activated, successful_exports.min(FREE_EXPORT_LIMIT), key),
    ) {
        eprintln!("Blaze: could not save license state: {error}");
    }
}

const PADDING_MS_VALUES: [f64; 3] = [120.0, 60.0, 20.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditingMode {
    Blaze,
    Precise,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolTab {
    ModeSelector,
    BlazeMode,
    PreciseMode,
    Export,
    License,
}

impl ToolTab {
    fn label(self) -> &'static str {
        match self {
            Self::ModeSelector => "Mode Selector",
            Self::BlazeMode => "Blaze Mode",
            Self::PreciseMode => "Precision Mode",
            Self::Export => "Export",
            Self::License => "License",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HelpTab { Guide, KeyboardMouse, Timeline, Blaze, Precision, Export, Activation }

impl HelpTab {
    fn label(self) -> &'static str {
        match self {
            Self::Guide => "Blaze Guide",
            Self::KeyboardMouse => "Keyboard & Mouse",
            Self::Timeline => "Timeline",
            Self::Blaze => "Blaze Mode",
            Self::Precision => "Precision Mode",
            Self::Export => "Export",
            Self::Activation => "Activation",
        }
    }
}




fn help_page_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    use egui::{FontFamily, FontId};
    ui.label(
        egui::RichText::new(title)
            .font(FontId::new(20.0, FontFamily::Proportional))
            .strong()
            .color(COLOR_TEXT),
    );
    ui.add_space(5.0);
    ui.label(
        egui::RichText::new(subtitle)
            .font(FontId::new(12.5, FontFamily::Proportional))
            .color(COLOR_MUTED),
    );
    ui.add_space(12.0);
}

fn help_section_label(ui: &mut egui::Ui, label: &str) {
    use egui::{FontFamily, FontId};
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(label)
            .font(FontId::new(10.5, FontFamily::Proportional))
            .strong()
            .color(COLOR_ACCENT_HOVER),
    );
    ui.add_space(7.0);
}

fn help_card(ui: &mut egui::Ui, title: &str, description: &str) {
    use egui::{FontFamily, FontId};

    let width = ui.available_width().max(120.0);
    let left_pad = 12.0;
    let right_pad = 12.0;
    let top_pad = 10.0;
    let bottom_pad = 10.0;
    let title_font = FontId::new(13.5, FontFamily::Proportional);
    let description_font = FontId::new(12.5, FontFamily::Proportional);
    let title_color = COLOR_TEXT;
    let description_color = COLOR_MUTED;

    let title_galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        title_font,
        title_color,
    );
    let description_galley = ui.painter().layout(
        description.to_owned(),
        description_font,
        description_color,
        (width - left_pad - right_pad).max(80.0),
    );
    let gap = 5.0;
    let height = top_pad
        + title_galley.size().y.max(18.0)
        + gap
        + description_galley.size().y
        + bottom_pad;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, height),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(7),
        COLOR_SURFACE,
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(7),
        egui::Stroke::new(1.0, COLOR_BORDER),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(rect.left() + left_pad, rect.top() + top_pad),
        title_galley,
        title_color,
    );
    painter.galley(
        egui::pos2(
            rect.left() + left_pad,
            rect.top() + top_pad + 18.0 + gap,
        ),
        description_galley,
        description_color,
    );
    ui.add_space(7.0);
}


fn copy_support_email(ui: &mut egui::Ui) {
    let copied_id = ui.scope_id().with("support-email-copied");
    let mut copied_recently = ui.ctx().data(|data| {
        data.get_temp::<Instant>(copied_id)
            .is_some_and(|time| time.elapsed() < Duration::from_secs(2))
    });

    ui.horizontal(|ui| {
        let response = ui.add(
            egui::Label::new(
                egui::RichText::new("workingankush369@gmail.com")
                    .color(COLOR_ACCENT_HOVER)
                    .underline(),
            )
            .sense(egui::Sense::click()),
        );
        if response.clicked() {
            ui.ctx().copy_text("workingankush369@gmail.com".to_owned());
            ui.ctx().data_mut(|data| data.insert_temp(copied_id, Instant::now()));
            copied_recently = true;
        }
        response.on_hover_text("Copy email address");

        if copied_recently {
            let (check_rect, _) =
                ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            let check_color = egui::Color32::from_rgb(105, 205, 130);
            let check_stroke = egui::Stroke::new(1.8, check_color);
            ui.painter().line_segment(
                [
                    egui::pos2(check_rect.left() + 1.0, check_rect.center().y),
                    egui::pos2(check_rect.left() + 4.5, check_rect.bottom() - 2.0),
                ],
                check_stroke,
            );
            ui.painter().line_segment(
                [
                    egui::pos2(check_rect.left() + 4.5, check_rect.bottom() - 2.0),
                    egui::pos2(check_rect.right() - 1.0, check_rect.top() + 2.0),
                ],
                check_stroke,
            );
            ui.label(
                egui::RichText::new("Copied!")
                    .size(11.5)
                    .strong()
                    .color(check_color),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }
    });
}

fn help_shortcut(ui: &mut egui::Ui, key: &str, description: &str) {
    use egui::{FontFamily, FontId};

    let width = ui.available_width().max(120.0);
    let left_pad = 12.0;
    let right_pad = 12.0;
    let key_column_width = 112.0;
    let column_gap = 12.0;
    let description_width =
        (width - left_pad - right_pad - key_column_width - column_gap).max(80.0);
    let description_font = FontId::new(12.5, FontFamily::Proportional);
    let description_color = ui.visuals().text_color();
    let description_galley = ui.painter().layout(
        description.to_owned(),
        description_font,
        description_color,
        description_width,
    );
    let row_height = description_galley.size().y.max(20.0) + 16.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, row_height),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(6),
        egui::Color32::from_rgb(16, 18, 20),
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, COLOR_BORDER),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(
            rect.left() + left_pad + key_column_width * 0.5,
            rect.center().y,
        ),
        egui::Align2::CENTER_CENTER,
        key,
        FontId::new(12.0, FontFamily::Monospace),
        egui::Color32::from_rgb(255, 143, 119),
    );
    painter.galley(
        egui::pos2(
            rect.left() + left_pad + key_column_width + column_gap,
            rect.center().y - description_galley.size().y * 0.5,
        ),
        description_galley,
        description_color,
    );
    ui.add_space(5.0);
}

fn slider_handle_double_clicked(
    ui: &egui::Ui,
    response: &egui::Response,
    value: f64,
    min: f64,
    max: f64,
) -> bool {
    // egui Slider uses Sense::drag(), not Sense::click(), so its Response does
    // not set the CLICKED flag required by Response::double_clicked().
    // Inspect the pointer double-click directly and constrain it to the handle.
    let rect = response.rect;
    let radius = rect.height() / 2.5;
    let travel = (rect.width() - 2.0 * radius).max(0.0);
    let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0) as f32;
    let handle_center = egui::pos2(
        rect.left() + radius + travel * fraction,
        rect.center().y,
    );

    ui.input(|input| {
        input.pointer.button_double_clicked(egui::PointerButton::Primary)
            && input.pointer.interact_pos().is_some_and(|pointer| {
                pointer.distance(handle_center) <= radius + 5.0
            })
    })
}

fn take_wheel_step(ui: &mut egui::Ui, response: &egui::Response) -> Option<f64> {
    if !response.hovered() { return None; }
    ui.input_mut(|input| {
        let delta: f32 = input.raw.events.iter().filter_map(|event| match event {
            egui::Event::MouseWheel { delta, .. } => Some(delta.y),
            _ => None,
        }).sum();

        // Always consume residual/kinetic vertical scrolling while the pointer
        // remains over this control. Only a fresh wheel event changes its value.
        input.smooth_scroll_delta.y = 0.0;
        if delta.abs() <= f32::EPSILON { return None; }
        Some(if delta < 0.0 { 1.0 } else { -1.0 })
    })
}

fn numeric_field_scope(
    ui: &mut egui::Ui,
    width: f32,
    add_field: impl FnOnce(&mut egui::Ui),
) {
    ui.scope(|ui| {
        ui.set_width(width);
        let style = ui.style_mut();
        style.spacing.item_spacing.x = 0.0;
        style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(27, 29, 32);
        style.visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 52, 57));
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(34, 36, 40);
        style.visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 133, 105));
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(39, 34, 33);
        style.visuals.widgets.active.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 104, 79));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_field(ui);
        });
    });
}

fn numeric_field_f64(
    ui: &mut egui::Ui, value: &mut f64, min: f64, max: f64,
    step: f64, decimals: usize, suffix: &str, width: f32, tooltip: &str,
) {
    let mut response = None;
    numeric_field_scope(ui, width, |ui| {
        response = Some(ui.add_sized(
            [width, 28.0],
            egui::DragValue::new(value)
                .range(min..=max)
                .speed(step)
                .fixed_decimals(decimals)
                .suffix(suffix)
                .update_while_editing(false)
                .clip_text(true),
        ).on_hover_text(tooltip));
    });
    if let Some(response) = response {
        if let Some(direction) = take_wheel_step(ui, &response) {
            *value = (*value + direction * step).clamp(min, max);
        }
    }
}

fn numeric_field_u32(
    ui: &mut egui::Ui, value: &mut u32, min: u32, max: u32, step: f64,
    prefix: &str, suffix: &str, width: f32, tooltip: &str,
) {
    let mut response = None;
    numeric_field_scope(ui, width, |ui| {
        response = Some(ui.add_sized(
            [width, 28.0],
            egui::DragValue::new(value)
                .range(min..=max)
                .speed(step)
                .fixed_decimals(0)
                .prefix(prefix)
                .suffix(suffix)
                .update_while_editing(false)
                .clip_text(true),
        ).on_hover_text(tooltip));
    });
    if let Some(response) = response {
        if let Some(direction) = take_wheel_step(ui, &response) {
            let next = *value as f64 + direction * step;
            *value = next.round().clamp(min as f64, max as f64) as u32;
        }
    }
}

fn preset_value_field(
    ui: &mut egui::Ui, value: &mut usize, choices: &'static [f64],
    suffix: &str, width: f32, tooltip: &str,
) {
    let mut response = None;
    numeric_field_scope(ui, width, |ui| {
        response = Some(ui.add_sized(
            [width, 28.0],
            egui::DragValue::new(value)
                .range(0..=choices.len().saturating_sub(1))
                .speed(1.0)
                .custom_formatter(move |index, _| {
                    let index = (index.round() as usize).min(choices.len() - 1);
                    format!("{:.0}", choices[index])
                })
                .custom_parser(move |text| {
                    let entered = text.trim().parse::<f64>().ok()?;
                    choices.iter().enumerate()
                        .min_by(|(_, a), (_, b)| {
                            (*a - entered).abs().total_cmp(&(*b - entered).abs())
                        })
                        .map(|(index, _)| index as f64)
                })
                .suffix(suffix)
                .update_while_editing(false)
                .clip_text(true),
        ).on_hover_text(tooltip));
    });
    if let Some(response) = response {
        if let Some(direction) = take_wheel_step(ui, &response) {
            let next = *value as f64 + direction;
            *value = next.round().clamp(0.0, choices.len().saturating_sub(1) as f64) as usize;
        }
    }
}

fn slider_control<T>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    description: Option<&str>,
    preset_labels: Option<(&[&str], usize)>,
    add_value: impl FnOnce(&mut egui::Ui, &mut T),
    add_slider: impl FnOnce(&mut egui::Ui, &mut T),
) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(23, 25, 28))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 50)))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).strong().size(13.0));
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| add_value(ui, value),
                );
            });

            if let Some(description) = description {
                ui.add_space(1.0);
                ui.label(
                    egui::RichText::new(description)
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            }

            ui.add_space(3.0);

            // Keep the slider's edge spacing identical in Normal and Expert
            // modes. Preset labels are laid out separately below; they must not
            // change the slider track's horizontal inset.
            let content_width = ui.available_width();
            let horizontal_inset = 6.0;
            let slider_width = (content_width - horizontal_inset * 2.0).max(1.0);

            let mut slider_visuals = ui.visuals().clone();
            slider_visuals.handle_shape = egui::style::HandleShape::Circle;
            slider_visuals.slider_trailing_fill = true;
            slider_visuals.selection.bg_fill = egui::Color32::from_rgb(255, 104, 79);
            slider_visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(55, 59, 64);
            slider_visuals.widgets.inactive.fg_stroke =
                egui::Stroke::new(1.5, egui::Color32::from_rgb(210, 213, 216));
            slider_visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(67, 71, 77);
            slider_visuals.widgets.hovered.fg_stroke =
                egui::Stroke::new(1.7, egui::Color32::from_rgb(255, 153, 132));
            slider_visuals.widgets.active.bg_fill = egui::Color32::from_rgb(255, 104, 79);
            slider_visuals.widgets.active.fg_stroke =
                egui::Stroke::new(1.7, egui::Color32::WHITE);

            ui.horizontal(|row| {
                row.add_space(horizontal_inset);
                row.allocate_ui_with_layout(
                    egui::vec2(slider_width, 32.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |slider_ui| {
                        slider_ui.spacing_mut().slider_width = slider_width;
                        slider_ui.spacing_mut().slider_rail_height = 5.0;
                        *slider_ui.visuals_mut() = slider_visuals;
                        add_slider(slider_ui, value);
                    },
                );
            });

            if let Some((labels, selected)) = preset_labels {
                preset_scale_labels(ui, labels, selected);
            }
        });
}

fn padding_control_row(
    ui: &mut egui::Ui,
    side: &str,
    value: &mut u32,
    tooltip: &str,
) {
    ui.horizontal(|row| {
        row.spacing_mut().item_spacing.x = 7.0;
        let (badge_rect, _) = row.allocate_exact_size(egui::vec2(25.0, 28.0), egui::Sense::hover());
        row.painter().rect_filled(badge_rect, egui::CornerRadius::same(5), egui::Color32::from_rgb(31, 34, 37));
        row.painter().rect_stroke(badge_rect, egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 58, 63)), egui::StrokeKind::Inside);
        row.painter().text(badge_rect.center(), egui::Align2::CENTER_CENTER, side,
            egui::FontId::proportional(11.5), egui::Color32::from_rgb(220, 223, 226));

        let field_width = 64.0;
        let key_width = 15.0;
        let slider_width = (row.available_width() - field_width - key_width - 8.0).max(60.0);
        let mut slider_response = None;
        row.scope(|slider_ui| {
            slider_ui.spacing_mut().slider_width = slider_width;
            slider_response = Some(slider_ui.add_sized(
                [slider_width, 28.0],
                egui::Slider::new(value, 0..=2000).show_value(false).trailing_fill(true),
            ).on_hover_text("Drag to adjust · double-click to reset"));
        });
        if let Some(response) = slider_response {
            if slider_handle_double_clicked(row, &response, *value as f64, 0.0, 2000.0) {
                *value = 60;
            } else if let Some(direction) = take_wheel_step(row, &response) {
                *value = (*value as f64 + direction * 10.0).round().clamp(0.0, 2000.0) as u32;
            }
        }

        let (keys_rect, _) = row.allocate_exact_size(egui::vec2(key_width, 28.0), egui::Sense::hover());
        let up = egui::Rect::from_min_size(keys_rect.min, egui::vec2(15.0, 13.0));
        let down = egui::Rect::from_min_size(
            egui::pos2(keys_rect.left(), keys_rect.top() + 15.0),
            egui::vec2(15.0, 13.0),
        );
        egui::Image::new(egui::include_image!("../assets/key-up.svg"))
            .fit_to_exact_size(up.size())
            .paint_at(row, up);
        egui::Image::new(egui::include_image!("../assets/key-down.svg"))
            .fit_to_exact_size(down.size())
            .paint_at(row, down);

        numeric_field_u32(row, value, 0, 2000, 10.0, "", " ms", field_width, tooltip);
    });
}

fn mode_selection_card(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    icon: ModeCardIcon,
    card_height: f32,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, card_height), egui::Sense::click());

    let visuals = ui.visuals();
    let accent = match icon {
        ModeCardIcon::Blaze => egui::Color32::from_rgb(255, 120, 92),
        ModeCardIcon::Precision => egui::Color32::from_rgb(255, 120, 92),
    };
    let fill = if !enabled {
        egui::Color32::from_rgb(25, 26, 28)
    } else if selected {
        egui::Color32::from_rgb(43, 34, 32)
    } else if response.hovered() {
        egui::Color32::from_rgb(39, 41, 44)
    } else {
        egui::Color32::from_rgb(30, 32, 35)
    };
    let border = if selected {
        accent
    } else if response.hovered() && enabled {
        egui::Color32::from_rgb(91, 94, 98)
    } else {
        egui::Color32::from_rgb(57, 59, 63)
    };

    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(8), fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(8),
        egui::Stroke::new(if selected { 1.5 } else { 1.0 }, border),
        egui::StrokeKind::Inside,
    );

    let icon_color = if enabled {
        accent
    } else {
        visuals.weak_text_color()
    };
    let title_color = if enabled {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    let description_color = visuals.weak_text_color();

    let title_galley = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(15.0),
        title_color,
    );
    let group_width = 28.0 + 10.0 + title_galley.size().x;
    let group_left = rect.center().x - group_width * 0.5;
    let icon_center = egui::pos2(group_left + 14.0, rect.top() + card_height * 0.36);
    let icon_source = match icon {
        ModeCardIcon::Blaze => egui::include_image!("../assets/mode-blaze-bolt.svg"),
        ModeCardIcon::Precision => egui::include_image!("../assets/precision-target.svg"),
    };
    egui::Image::new(icon_source)
        .fit_to_exact_size(egui::vec2(25.0, 25.0))
        .tint(icon_color)
        .paint_at(
            ui,
            egui::Rect::from_center_size(icon_center, egui::vec2(25.0, 25.0)),
        );

    painter.galley(
        egui::pos2(
            group_left + 38.0,
            icon_center.y - title_galley.size().y * 0.5,
        ),
        title_galley,
        title_color,
    );

    let description_galley = painter.layout(
        description.to_owned(),
        egui::FontId::proportional(12.0),
        description_color,
        (rect.width() - 30.0).max(100.0),
    );
    painter.galley(
        egui::pos2(
            rect.center().x - description_galley.size().x * 0.5,
            rect.top() + card_height * 0.56,
        ),
        description_galley,
        description_color,
    );

    if selected {
        painter.circle_filled(
            egui::pos2(rect.right() - 14.0, rect.top() + 14.0),
            3.5,
            accent,
        );
    }

    response.on_hover_text(if enabled {
        format!("Select {title}")
    } else {
        "Blaze Mode is unavailable until keyframe analysis completes.".to_owned()
    })
}

#[derive(Clone, Copy)]
enum ModeCardIcon {
    Blaze,
    Precision,
}

fn preset_scale_labels(ui: &mut egui::Ui, labels: &[&str], selected: usize) {
    ui.columns(labels.len(), |columns| {
        for (index, label) in labels.iter().enumerate() {
            let color = if index == selected {
                columns[index].visuals().text_color()
            } else {
                columns[index].visuals().weak_text_color()
            };
            columns[index].with_layout(
                egui::Layout::top_down(egui::Align::Center),
                |column| {
                    column.add(
                        egui::Label::new(egui::RichText::new(*label).size(11.5).color(color))
                            .wrap()
                            .halign(egui::Align::Center),
                    );
                },
            );
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditRegion {
    start: f64,
    end: f64,
}

type ProbeResult = (u64, Result<(PathBuf, MediaInfo), String>);
type SilenceResult = (u64, Result<Vec<SilenceInterval>, String>);
type KeyframeResult = (u64, Result<Vec<f64>, String>);
type ExportResult = (u64, Result<String, String>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LicensePlan {
    Monthly,
    Permanent,
}

struct LicenseEntitlement {
    plan: LicensePlan,
    valid_until: Option<String>,
}

type LicenseResult = Result<LicenseEntitlement, String>;
enum WaveformMessage {
    Progress { generation: u64, fraction: f32 },
    Complete { generation: u64, result: Result<Vec<u8>, String> },
}

struct BlazeApp {
    playback: Playback,
    selected_path: Option<PathBuf>,
    media_info: Option<MediaInfo>,
    loading: bool,
    seeking: bool,
    playback_position: f64,
    probe_generation: u64,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
    waveform: Option<Waveform>,
    waveform_tx: Sender<WaveformMessage>,
    waveform_rx: Receiver<WaveformMessage>,
    waveform_progress: Option<f32>,
    silence_tx: Sender<SilenceResult>,
    silence_rx: Receiver<SilenceResult>,
    silence_generation: u64,
    silence_running: bool,
    keyframe_tx: Sender<KeyframeResult>,
    keyframe_rx: Receiver<KeyframeResult>,
    keyframe_generation: u64,
    keyframes: Vec<f64>,
    keyframes_running: bool,
    keyframe_error: Option<String>,
    export_tx: Sender<ExportResult>,
    export_rx: Receiver<ExportResult>,
    export_generation: u64,
    export_running: bool,
    license_tx: Sender<LicenseResult>,
    license_rx: Receiver<LicenseResult>,
    license_verifying: bool,
    license_key: Option<String>,
    license_activation_pending: bool,
    last_license_check: Instant,
    export_status: Option<(bool, String)>,
    detection_level: usize,
    padding_level: usize,
    minimum_silence_ms: u32,
    expert_mode: bool,
    custom_padding_before_ms: u32,
    custom_padding_after_ms: u32,
    padding_linked: bool,
    custom_threshold_db: f64,
    custom_peak_gate_enabled: bool,
    custom_peak_ceiling_dbfs: f64,
    timeline_zoom: f32,
    timeline_offset: f64,
    edit_regions: Vec<EditRegion>,
    pending_region_start: Option<f64>,
    skip_cuts_enabled: bool,
    active_mode: Option<EditingMode>,
    active_tool_tab: ToolTab,
    show_help: bool,
    help_tab: HelpTab,
    pending_preview_fit: bool,
    show_license_dialog: bool,
    license_key_input: String,
    license_notice: Option<String>,
    license_activated: bool,
    license_plan: Option<LicensePlan>,
    license_valid_until: Option<String>,
    successful_exports: u32,
}

fn nearest_keyframe(keyframes: &[f64], time: f64) -> Option<f64> {
    keyframes.iter().copied().min_by(|a, b| {
        (a - time).abs().total_cmp(&(b - time).abs())
    })
}

fn nearest_boundary_keyframe(keyframes: &[f64], time: f64, other: f64, is_start: bool) -> Option<f64> {
    keyframes.iter().copied()
        .filter(|candidate| if is_start { *candidate < other } else { *candidate > other })
        .min_by(|a, b| (a - time).abs().total_cmp(&(b - time).abs()))
}

impl BlazeApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        egui_extras::install_image_loaders(&cc.egui_ctx);

        // Apply the shared website/application design tokens.
        apply_blaze_theme(&cc.egui_ctx);
        cc.egui_ctx.send_viewport_cmd(
            egui::ViewportCommand::Title(APP_TITLE.to_owned()),
        );

        let playback = Playback::new(cc)?;
        let (probe_tx, probe_rx) = mpsc::channel();
        let (waveform_tx, waveform_rx) = mpsc::channel();
        let (silence_tx, silence_rx) = mpsc::channel();
        let (keyframe_tx, keyframe_rx) = mpsc::channel();
        let (export_tx, export_rx) = mpsc::channel();
        let (license_tx, license_rx) = mpsc::channel();
        let (successful_exports, license_key) = load_license_state();

        Ok(Self {
            playback,
            selected_path: None,
            media_info: None,
            loading: false,
            seeking: false,
            playback_position: 0.0,
            probe_generation: 0,
            probe_tx,
            probe_rx,
            waveform: None,
            waveform_tx,
            waveform_rx,
            waveform_progress: None,
            silence_tx,
            silence_rx,
            silence_generation: 0,
            silence_running: false,
            keyframe_tx,
            keyframe_rx,
            keyframe_generation: 0,
            keyframes: Vec::new(),
            keyframes_running: false,
            keyframe_error: None,
            export_tx,
            export_rx,
            export_generation: 0,
            export_running: false,
            license_tx,
            license_rx,
            license_verifying: false,
            license_key,
            license_activation_pending: false,
            last_license_check: Instant::now() - Duration::from_secs(60),
            export_status: None,
            detection_level: 2,
            padding_level: 1,
            minimum_silence_ms: 300,
            expert_mode: false,
            custom_padding_before_ms: 60,
            custom_padding_after_ms: 60,
            padding_linked: false,
            custom_threshold_db: 22.0,
            custom_peak_gate_enabled: false,
            custom_peak_ceiling_dbfs: -35.0,
            timeline_zoom: 1.0,
            timeline_offset: 0.0,
            edit_regions: Vec::new(),
            pending_region_start: None,
            skip_cuts_enabled: false,
            active_mode: None,
            active_tool_tab: ToolTab::ModeSelector,
            show_help: true,
            help_tab: HelpTab::Guide,
            pending_preview_fit: false,
            show_license_dialog: false,
            license_key_input: String::new(),
            license_notice: None,
            license_activated: false,
            license_plan: None,
            license_valid_until: None,
            successful_exports,
        })
    }

    fn start_keyframe_analysis(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.keyframe_generation = self.keyframe_generation.wrapping_add(1);
        let generation = self.keyframe_generation;
        self.keyframes.clear();
        self.keyframes_running = true;
        self.keyframe_error = None;
        let sender = self.keyframe_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = engine.detect_keyframes(&path.to_string_lossy());
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_keyframes(&mut self, ctx: &egui::Context) {
        while let Ok((generation, result)) = self.keyframe_rx.try_recv() {
            if generation != self.keyframe_generation { continue; }
            self.keyframes_running = false;
            match result {
                Ok(mut keyframes) => {
                    keyframes.sort_by(f64::total_cmp);
                    keyframes.dedup_by(|a, b| (*a - *b).abs() < 0.000_001);
                    self.keyframes = keyframes;
                    if self.keyframes.is_empty() {
                        self.keyframe_error = Some("No usable video keyframes were found.".to_string());
                    }
                }
                Err(error) => self.keyframe_error = Some(error),
            }
            ctx.request_repaint();
        }
    }

    fn start_silence_detection(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_path.clone() else { return; };
        if self.silence_running { return; }

        self.silence_generation = self.silence_generation.wrapping_add(1);
        let generation = self.silence_generation;
        self.silence_running = true;

        let level = match self.detection_level {
            0 => DetectionLevel::Comfortable,
            1 => DetectionLevel::Optimal,
            2 => DetectionLevel::Default,
            3 => DetectionLevel::Tight,
            _ => DetectionLevel::Aggressive,
        };
        let preset_padding = match self.padding_level {
            0 => PaddingPreset::Comfortable.milliseconds(),
            2 => PaddingPreset::Tight.milliseconds(),
            _ => PaddingPreset::Normal.milliseconds(),
        };
        let (padding_before_ms, padding_after_ms) = if self.expert_mode {
            (self.custom_padding_before_ms, self.custom_padding_after_ms)
        } else {
            (preset_padding, preset_padding)
        };
        let config = SilenceDetectionConfig {
            level,
            padding_before_ms,
            padding_after_ms,
            minimum_duration_ms: self.minimum_silence_ms,
            threshold_below_baseline_db: self.expert_mode.then_some(self.custom_threshold_db),
            peak_ceiling_dbfs: (self.expert_mode && self.custom_peak_gate_enabled)
                .then_some(self.custom_peak_ceiling_dbfs),
        };
        let tx = self.silence_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = blaze_engine::silence::detect_silence(&engine, &path.to_string_lossy(), config);
            let _ = tx.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_silence(&mut self) {
        while let Ok((generation, result)) = self.silence_rx.try_recv() {
            if generation != self.silence_generation { continue; }
            self.silence_running = false;
            match result {
                Ok(intervals) => {
                    // Silence detection is identical in both modes. Blaze applies
                    // keyframe constraints only when converting detections to cuts.
                    self.edit_regions = if self.active_mode == Some(EditingMode::Blaze) {
                        let keyframes = &self.keyframes;
                        intervals.into_iter().filter_map(|interval| {
                            let start = nearest_keyframe(keyframes, interval.start)?;
                            let end = nearest_boundary_keyframe(keyframes, interval.end, start, false)?;
                            (end > start).then_some(EditRegion { start, end })
                        }).collect()
                    } else {
                        intervals.into_iter()
                            .filter(|interval| interval.end > interval.start)
                            .map(|interval| EditRegion { start: interval.start, end: interval.end })
                            .collect()
                    };
                    self.pending_region_start = None;
                }
                Err(error) => eprintln!("Blaze: silence detection failed: {error}"),
            }
        }
    }

    fn media_duration(&self) -> f64 {
        self.media_info
            .as_ref()
            .map(|info| info.duration)
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .unwrap_or_else(|| self.playback.duration())
            .max(0.01)
    }

    fn open_video(&mut self, ctx: &egui::Context) {
        let Some(path) = FileDialog::new().pick_file() else {
            return;
        };

        self.probe_generation = self.probe_generation.wrapping_add(1);
        self.keyframe_generation = self.keyframe_generation.wrapping_add(1);
        self.keyframes.clear();
        self.keyframes_running = false;
        self.keyframe_error = None;
        self.silence_generation = self.silence_generation.wrapping_add(1);
        self.silence_running = false;
        let generation = self.probe_generation;
        self.loading = true;
        self.seeking = false;
        self.playback_position = 0.0;
        self.timeline_zoom = 1.0;
        self.timeline_offset = 0.0;
        self.edit_regions.clear();
        self.pending_region_start = None;
        self.media_info = None;
        self.waveform = None;
        self.waveform_progress = Some(0.0);
        self.selected_path = Some(path.clone());
        self.pending_preview_fit = false;
        self.start_keyframe_analysis(path.clone(), ctx);

        if let Err(error) = self.playback.load(&path) {
            eprintln!("Blaze: {error}");
            self.playback.stop();
            self.selected_path = None;
            self.loading = false;
            self.waveform_progress = None;
            return;
        }

        let sender = self.probe_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = engine
                .probe_media(&path.to_string_lossy())
                .map(|info| (path, info));
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_probe(&mut self, ctx: &egui::Context) {
        while let Ok((generation, result)) = self.probe_rx.try_recv() {
            if generation != self.probe_generation {
                continue;
            }

            match result {
                Ok((path, info)) => {
                    self.selected_path = Some(path.clone());
                    self.loading = false;

                    if info.has_audio {
                        let channels = info.audio_channels.unwrap_or(2).clamp(1, 32) as usize;
                        let duration = info.duration;
                        let sender = self.waveform_tx.clone();
                        let repaint = ctx.clone();
                        std::thread::spawn(move || {
                            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
                            let engine = MediaEngine::new([tools]);
                            let result = engine.generate_waveform_with_progress(
                                &path.to_string_lossy(),
                                channels,
                                duration,
                                |fraction| {
                                    let _ = sender.send(WaveformMessage::Progress {
                                        generation,
                                        fraction,
                                    });
                                    repaint.request_repaint();
                                },
                            );
                            let _ = sender.send(WaveformMessage::Complete { generation, result });
                            repaint.request_repaint();
                        });
                    } else {
                        self.waveform_progress = None;
                    }

                    self.media_info = Some(info);
                    self.pending_preview_fit = true;
                }
                Err(error) => {
                    self.loading = false;
                    self.selected_path = None;
                    self.media_info = None;
                    self.waveform = None;
                    self.waveform_progress = None;
                    self.playback_position = 0.0;
                    self.playback.stop();
                    eprintln!("Blaze: media analysis failed: {error}");
                }
            }
        }
    }

    fn poll_waveform(&mut self) {
        while let Ok(message) = self.waveform_rx.try_recv() {
            match message {
                WaveformMessage::Progress { generation, fraction }
                    if generation == self.probe_generation =>
                {
                    self.waveform_progress = Some(fraction.clamp(0.0, 1.0));
                }
                WaveformMessage::Complete { generation, result }
                    if generation == self.probe_generation =>
                {
                    self.waveform_progress = None;
                    match result {
                        Ok(bytes) => match Waveform::from_bytes(&bytes) {
                            Ok(waveform) => self.waveform = Some(waveform),
                            Err(error) => {
                                self.waveform = None;
                                eprintln!("Blaze: waveform data was invalid: {error}");
                            }
                        },
                        Err(error) => {
                            self.waveform = None;
                            eprintln!("Blaze: waveform generation failed: {error}");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn can_export(&self) -> bool {
        self.license_activated || self.successful_exports < FREE_EXPORT_LIMIT
    }

    fn activate_license_key(&mut self, ctx: &egui::Context) {
        let key = self.license_key_input.trim().to_owned();
        if key.is_empty() || self.license_verifying || self.license_activated {
            return;
        }
        self.license_verifying = true;
        self.license_activation_pending = true;
        self.license_key = Some(key.clone());
        self.last_license_check = Instant::now();
        self.license_notice = Some("Checking your Blaze licence…".to_owned());
        let sender = self.license_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verify_gumroad_license(&key, true);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_license_verification(&mut self) {
        while let Ok(result) = self.license_rx.try_recv() {
            self.license_verifying = false;
            match result {
                Ok(entitlement) => {
                    self.license_activated = true;
                    self.license_plan = Some(entitlement.plan);
                    self.license_valid_until = entitlement.valid_until;
                    if self.license_activation_pending {
                        self.license_notice = Some(match entitlement.plan {
                            LicensePlan::Monthly => "Blaze Monthly is activated. Cut away!".to_owned(),
                            LicensePlan::Permanent => "Blaze Permanent is activated. Cut away!".to_owned(),
                        });
                        self.license_key_input.clear();
                    }
                    self.license_activation_pending = false;
                    persist_license_state(self.license_activated, self.successful_exports, self.license_key.as_deref());
                }
                Err(message) => {
                    self.license_activated = false;
                    self.license_plan = None;
                    self.license_valid_until = None;
                    let activation_failed = self.license_activation_pending;
                    self.license_activation_pending = false;
                    if activation_failed {
                        self.license_key = None;
                    }
                    self.license_notice = Some(message);
                    persist_license_state(false, self.successful_exports, self.license_key.as_deref());
                }
            }
        }
    }

    fn start_blaze_export(&mut self, output: PathBuf, ctx: &egui::Context) {
        if self.export_running || !self.can_export() { return; }
        let license_key = self.license_key.clone();
        let (Some(source), Some(info)) = (self.selected_path.clone(), self.media_info.clone()) else { return; };
        let ranges: Vec<(f64, f64)> = self.edit_regions.iter().map(|r| (r.start, r.end)).collect();
        let duration = info.duration;
        self.export_generation = self.export_generation.wrapping_add(1);
        let generation = self.export_generation;
        self.export_running = true;
        self.export_status = None;
        let sender = self.export_tx.clone();
        let repaint = ctx.clone();

        std::thread::spawn(move || {
            let result = if let Some(key) = license_key.as_deref() {
                verify_gumroad_license(key, false).and_then(|_| {
                    let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
                    let engine = MediaEngine::new([tools]);
                    export::export_blaze_video(&engine, &source, &output, duration, &ranges)
                })
            } else {
                let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
                let engine = MediaEngine::new([tools]);
                export::export_blaze_video(&engine, &source, &output, duration, &ranges)
            }
                .map(|report| {
                    let mut message = format!(
                        "Blaze Export completed.\n{}\nDuration: {:.2}s",
                        report.output_path.display(),
                        report.actual_duration
                    );
                    if let Some(warning) = report.warning {
                        message.push_str("\n");
                        message.push_str(&warning);
                    }
                    message
                });
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn start_interchange_export(
        &mut self,
        format: InterchangeFormat,
        output: PathBuf,
        ctx: &egui::Context,
    ) {
        if self.export_running || !self.can_export() { return; }
        let license_key = self.license_key.clone();
        let (Some(source), Some(info)) = (self.selected_path.clone(), self.media_info.clone()) else { return; };
        let ranges: Vec<(f64, f64)> = self.edit_regions.iter().map(|r| (r.start, r.end)).collect();
        self.export_generation = self.export_generation.wrapping_add(1);
        let generation = self.export_generation;
        self.export_running = true;
        self.export_status = None;
        let sender = self.export_tx.clone();
        let repaint = ctx.clone();

        std::thread::spawn(move || {
            let result = if let Some(key) = license_key.as_deref() {
                verify_gumroad_license(key, false).and_then(|_| {
                    export::export_interchange(format, &source, &info, &ranges, &output)
                })
            } else {
                export::export_interchange(format, &source, &info, &ranges, &output)
            }.map(|_| format!("Timeline exported:\n{}", output.display()));
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_exports(&mut self) {
        while let Ok((generation, result)) = self.export_rx.try_recv() {
            if generation != self.export_generation { continue; }
            self.export_running = false;
            self.export_status = Some(match result {
                Ok(message) => {
                    if !self.license_activated {
                        self.successful_exports =
                            self.successful_exports.saturating_add(1).min(FREE_EXPORT_LIMIT);
                        persist_license_state(self.license_activated, self.successful_exports, self.license_key.as_deref());
                    }
                    (true, message)
                }
                Err(error) => (false, error),
            });
        }
    }

}

impl eframe::App for BlazeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_probe(ctx);
        self.poll_waveform();
        self.poll_silence();
        self.poll_keyframes(ctx);
        self.poll_exports();
        self.poll_license_verification();
        ctx.request_repaint_after(Duration::from_secs(60));

        // Revalidate activated installations at startup and every 60 seconds.
        if self.license_key.is_some()
            && !self.license_verifying
            && self.last_license_check.elapsed() >= Duration::from_secs(60)
        {
            let key = self.license_key.clone().unwrap_or_default();
            self.license_verifying = true;
            self.license_activation_pending = false;
            self.last_license_check = Instant::now();
            let sender = self.license_tx.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = verify_gumroad_license(&key, false);
                let _ = sender.send(result);
                repaint.request_repaint();
            });
        }

        // Global transport shortcut: handle Space at the app level rather
        // than inside a particular panel, so it works regardless of focus
        // or which part of the interface the user is interacting with.
        let space_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Space)
        });
        let help_shortcut = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::H)
        });
        let mark_start_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::S)
        });
        let mark_end_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::D)
        });
        let blaze_mode = self.active_mode == Some(EditingMode::Blaze);
        if self.selected_path.is_some() && self.active_mode.is_some() && mark_start_pressed {
            if self.pending_region_start.is_some() {
                self.pending_region_start = None;
            } else {
                if blaze_mode {
                    if let Some(start) = nearest_keyframe(&self.keyframes, self.playback_position) {
                        self.pending_region_start = Some(start);
                    }
                } else {
                    self.pending_region_start = Some(self.playback_position);
                }
            }
        }
        if self.selected_path.is_some() && self.active_mode.is_some() && mark_end_pressed {
            if let Some(start) = self.pending_region_start {
                let end = if blaze_mode {
                    nearest_boundary_keyframe(&self.keyframes, self.playback_position, start, false)
                } else { Some(self.playback_position) };
                if end.is_some_and(|end| end > start) {
                    self.edit_regions.push(EditRegion { start, end: end.unwrap() });
                    self.pending_region_start = None;
                }
            }
        }
        if help_shortcut {
            self.show_help = !self.show_help;
        }
        if space_pressed && self.selected_path.is_some() {
            if let Err(error) = self.playback.toggle_pause() {
                eprintln!("Blaze: {error}");
            }
        }

        if let Some(error) = self.playback.poll_error() {
            eprintln!("Blaze: {error}");
            self.loading = false;
            self.selected_path = None;
            self.media_info = None;
            self.waveform = None;
            self.waveform_progress = None;
            self.playback_position = 0.0;
        }

        if self.selected_path.is_some() && !self.seeking {
            self.playback_position = self.playback.position();

            // Skip completed S–D regions during active playback. Extend the target
            // through overlapping or directly adjacent regions so no marked content
            // is played between consecutive cuts.
            if self.skip_cuts_enabled && !self.playback.is_paused() {
                let mut skip_target = self.playback_position;
                loop {
                    let next_target = self
                        .edit_regions
                        .iter()
                        .filter(|region| {
                            region.start <= skip_target && region.end > skip_target
                        })
                        .map(|region| region.end)
                        .fold(skip_target, f64::max);

                    if next_target <= skip_target {
                        break;
                    }
                    skip_target = next_target;
                }

                if skip_target > self.playback_position {
                    if let Err(error) = self.playback.seek(skip_target) {
                        eprintln!("Blaze: could not skip cut region: {error}");
                    } else {
                        self.playback_position = skip_target;
                    }
                }
            }

            // Keep the playhead visible while playing. If playback starts outside
            // the zoomed viewport, jump to it; once it reaches the right edge,
            // advance the viewport while keeping the playhead comfortably in view.
            if !self.playback.is_paused() {
                let duration = self.media_duration();
                let visible_duration = duration / self.timeline_zoom as f64;
                let view_end = self.timeline_offset + visible_duration;
                if self.playback_position < self.timeline_offset
                    || self.playback_position > view_end
                {
                    self.timeline_offset = (self.playback_position - visible_duration * 0.8)
                        .clamp(0.0, (duration - visible_duration).max(0.0));
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        if self.loading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Make panel splitters easier to grab without changing their saved sizes.
        ui.style_mut().interaction.resize_grab_radius_side = 10.0;
        let root_rect = ui.available_rect_before_wrap();
        let tools_panel_id = ui.scope_id().with("tools");
        // The Tools controls need enough horizontal room for their labels,
        // values, and sliders. Keep that as the minimum, while allowing the
        // splitter to leave as little as 240 px for the preview at its maximum.
        let tools_min_width = 420.0_f32.min(root_rect.width().max(0.0));
        let tools_max_width = (root_rect.width() - 240.0)
            .max(tools_min_width);

        // Fit the initial preview width to the imported video's aspect ratio.
        // This runs once per import; later manual resizing remains untouched.
        if self.pending_preview_fit {
            if let Some(info) = self.media_info.as_ref() {
                let tools_width = default_tools_width(root_rect, info)
                    .clamp(tools_min_width, tools_max_width);
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(tools_width, root_rect.height()),
                );
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        tools_panel_id,
                        egui::containers::panel::PanelState {
                            outer_rect: panel_rect,
                        },
                    );
                });
                self.pending_preview_fit = false;
            }
        }

        // Panel::right can otherwise persist a wider content-measured rect after
        // Expert Mode lays out its controls. Clamp stale sizes, then use the
        // user's saved width as this frame's actual maximum. The custom splitter
        // below is the only code allowed to change that saved width.
        let saved_tools_width = egui::containers::panel::PanelState::load(
            ui.ctx(),
            tools_panel_id,
        )
        .map(|state| state.outer_rect.width())
        .map(|width| if (width - 380.0).abs() < 1.0 || (width - 420.0).abs() < 1.0 { 460.0 } else { width })
        .unwrap_or(460.0)
        .clamp(tools_min_width, tools_max_width);
        let normalized_tools_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(saved_tools_width, root_rect.height()),
        );
        ui.ctx().data_mut(|data| {
            data.insert_persisted(
                tools_panel_id,
                egui::containers::panel::PanelState {
                    outer_rect: normalized_tools_rect,
                },
            );
        });

        let timeline_panel = egui::Panel::bottom("timeline")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(10, 11, 12))
                    .stroke(egui::Stroke::NONE)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show_separator_line(false)
            .resizable(false)
            .default_size(220.0)
            .min_size(140.0)
            .show(ui, |ui| {
                let available = ui.available_rect_before_wrap();
                let ruler = egui::Rect::from_min_max(
                    egui::pos2(available.left() + 10.0, available.top() + 3.0),
                    egui::pos2(available.right() - 10.0, available.top() + 27.0),
                );
                let lane = egui::Rect::from_min_max(
                    egui::pos2(available.left() + 10.0, ruler.bottom() + 3.0),
                    egui::pos2(available.right() - 10.0, available.bottom() - 12.0),
                );

                let duration = self.media_duration();

                let can_seek = self.selected_path.is_some() && self.waveform_progress.is_none();
                let sense = if can_seek {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::hover()
                };
                let response = ui.allocate_rect(lane, sense);
                let ruler_response = ui.allocate_rect(ruler, sense);

                // Ctrl+wheel zooms; ordinary wheel input pans the timeline
                // horizontally. Horizontal trackpad input is also supported.
                if can_seek && (response.hovered() || ruler_response.hovered()) {
                    let (scroll, zoom_delta) = ui.input(|input| {
                        (input.smooth_scroll_delta, input.zoom_delta())
                    });
                    let visible_duration = (duration / self.timeline_zoom as f64).max(0.01);

                    // egui consumes Ctrl+wheel as its native zoom gesture and
                    // exposes it through zoom_delta(), not smooth_scroll_delta.
                    // Use that signal directly; it is 1.0 when no zoom gesture
                    // occurred (including ordinary wheel scrolling).
                    if (zoom_delta - 1.0).abs() > 0.0001 {
                        let pointer_x = ui.input(|input| input.pointer.hover_pos())
                            .map(|position| position.x)
                            .unwrap_or_else(|| lane.center().x);
                        let anchor = ((pointer_x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64;
                        let anchor_time = self.timeline_offset + anchor * visible_duration;
                        self.timeline_zoom = (self.timeline_zoom * zoom_delta).clamp(1.0, 128.0);
                        let new_visible = duration / self.timeline_zoom as f64;
                        self.timeline_offset = anchor_time - anchor * new_visible;
                    } else {
                        let pan_delta = if scroll.x.abs() > 0.0 {
                            scroll.x
                        } else {
                            scroll.y
                        };
                        if pan_delta.abs() > 0.0 {
                            self.timeline_offset -= pan_delta as f64 * visible_duration * 0.0015;
                        }
                    }

                    let visible_duration = duration / self.timeline_zoom as f64;
                    self.timeline_offset = self.timeline_offset.clamp(0.0, (duration - visible_duration).max(0.0));
                }

                let visible_duration = duration / self.timeline_zoom as f64;
                let view_start = self.timeline_offset / duration;
                let view_end = ((self.timeline_offset + visible_duration) / duration).min(1.0);

                // Adaptive time ruler: choose a clean 1–2–5 interval based on
                // available pixels, so labels remain legible at every zoom level.
                let raw_step = (visible_duration * 105.0 / lane.width().max(1.0) as f64).max(0.001);
                let magnitude = 10.0_f64.powf(raw_step.log10().floor());
                let normalized = raw_step / magnitude;
                let step = if normalized <= 1.0 { magnitude }
                    else if normalized <= 2.0 { 2.0 * magnitude }
                    else if normalized <= 5.0 { 5.0 * magnitude }
                    else { 10.0 * magnitude };
                let first_tick = (self.timeline_offset / step).ceil() * step;
                let mut tick = first_tick;
                while tick <= (self.timeline_offset + visible_duration).min(duration) + step * 0.0001 {
                    let x = lane.left()
                        + (((tick - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    if x >= ruler.left() && x <= ruler.right() {
                        ui.painter().line_segment(
                            [egui::pos2(x, ruler.bottom() - 5.0), egui::pos2(x, ruler.bottom())],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(91, 94, 98)),
                        );
                        ui.painter().text(
                            egui::pos2(x + 4.0, ruler.top() + 3.0),
                            egui::Align2::LEFT_TOP,
                            format_ruler_time(tick, step),
                            egui::FontId::proportional(11.5),
                            egui::Color32::from_rgb(145, 149, 153),
                        );
                    }
                    tick += step;
                }
                ui.painter().line_segment(
                    [ruler.left_bottom(), ruler.right_bottom()],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 49)),
                );

                if let Some(progress) = self.waveform_progress {
                    let fill_width = lane.width() * progress.clamp(0.0, 1.0);
                    if fill_width > 0.0 {
                        let fill = egui::Rect::from_min_size(
                            lane.min,
                            egui::vec2(fill_width, lane.height()),
                        );
                        ui.painter().rect_filled(
                            fill,
                            egui::CornerRadius::same(3),
                            egui::Color32::from_rgb(92, 45, 39),
                        );
                    }

                    let label = format!("Generating Waveform Data  ·  {}%", (progress * 100.0).round() as u32);
                    ui.painter().text(
                        lane.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(220, 220, 220),
                    );
                } else if let Some(waveform) = &self.waveform {
                    waveform.paint_range(ui.painter(), lane, view_start, view_end);
                } else {
                    ui.painter().line_segment(
                        [lane.left_center(), lane.right_center()],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 49)),
                    );
                }

                // Edit regions are independent of their origin (manual now,
                // silence analysis later). Their two boundary handles remain
                // freely movable in Precise Mode.
                let mut handle_interaction = false;
                let blaze_snapping = self.active_mode == Some(EditingMode::Blaze);
                let keyframes = self.keyframes.clone();
                for (index, region) in self.edit_regions.iter_mut().enumerate() {
                    let start_x = lane.left()
                        + (((region.start - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    let end_x = lane.left()
                        + (((region.end - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    let region_rect = egui::Rect::from_min_max(
                        egui::pos2(start_x, lane.top()),
                        egui::pos2(end_x, lane.bottom()),
                    );
                    if region_rect.intersects(lane) {
                        ui.painter().rect_filled(
                            region_rect.intersect(lane),
                            egui::CornerRadius::same(2),
                            egui::Color32::from_rgba_unmultiplied(255, 104, 79, 42),
                        );
                    }

                    for (is_start, x) in [(true, start_x), (false, end_x)] {
                        if x < lane.left() - 8.0 || x > lane.right() + 8.0 {
                            continue;
                        }
                        let handle_rect = egui::Rect::from_center_size(
                            egui::pos2(x, lane.center().y),
                            egui::vec2(14.0, lane.height().max(18.0)),
                        );
                        let id = ui.scope_id().with(("edit-region-handle", index, is_start));
                        let handle_response = ui.interact(handle_rect, id, egui::Sense::drag());
                        handle_interaction |= handle_response.hovered() || handle_response.dragged();
                        if handle_response.dragged() {
                            if let Some(pointer) = handle_response.interact_pointer_pos() {
                                let time = (self.timeline_offset
                                    + ((pointer.x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64
                                        * visible_duration)
                                    .clamp(0.0, duration);
                                if blaze_snapping {
                                    if let Some(snapped) = nearest_boundary_keyframe(&keyframes, time, if is_start { region.end } else { region.start }, is_start) {
                                        if is_start { region.start = snapped; } else { region.end = snapped; }
                                    }
                                } else if is_start {
                                    region.start = time.min(region.end - 0.001);
                                } else {
                                    region.end = time.max(region.start + 0.001).min(duration);
                                }
                            }
                        }
                        let color = if handle_response.hovered() || handle_response.dragged() {
                            egui::Color32::from_rgb(255, 151, 132)
                        } else {
                            egui::Color32::from_rgb(255, 104, 79)
                        };
                        ui.painter().line_segment(
                            [egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())],
                            egui::Stroke::new(if handle_response.hovered() { 4.0 } else { 3.0 }, color),
                        );
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(egui::pos2(x, lane.center().y), egui::vec2(7.0, 15.0)),
                            egui::CornerRadius::same(2),
                            color,
                        );
                    }
                }

                // An unpaired S marker is shown as a draggable starting jaw.
                if let Some(start) = self.pending_region_start {
                    let x = lane.left()
                        + (((start - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    if x >= lane.left() - 8.0 && x <= lane.right() + 8.0 {
                        let handle_rect = egui::Rect::from_center_size(
                            egui::pos2(x, lane.center().y),
                            egui::vec2(14.0, lane.height().max(18.0)),
                        );
                        let response = ui.interact(
                            handle_rect,
                            ui.scope_id().with("pending-region-start"),
                            egui::Sense::drag(),
                        );
                        handle_interaction |= response.hovered() || response.dragged();
                        if response.dragged() {
                            if let Some(pointer) = response.interact_pointer_pos() {
                                let time = (self.timeline_offset
                                    + ((pointer.x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64
                                        * visible_duration).clamp(0.0, duration);
                                self.pending_region_start = Some(if blaze_snapping {
                                    nearest_keyframe(&keyframes, time).unwrap_or(start)
                                } else { time });
                            }
                        }
                        ui.painter().line_segment(
                            [egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())],
                            egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 104, 79)),
                        );
                    }
                }

                if self.selected_path.is_some() && self.waveform_progress.is_none()
                    && self.playback_position >= self.timeline_offset
                    && self.playback_position <= self.timeline_offset + visible_duration
                {
                    let progress = ((self.playback_position - self.timeline_offset) / visible_duration) as f32;
                    let playhead_x = lane.left() + lane.width() * progress;
                    let playhead_color = egui::Color32::from_rgb(255, 104, 79);
                    ui.painter().line_segment(
                        [
                            egui::pos2(playhead_x, lane.top() + 7.0),
                            egui::pos2(playhead_x, lane.bottom()),
                        ],
                        egui::Stroke::new(1.5, playhead_color),
                    );
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(playhead_x - 6.0, lane.top() - 2.0),
                            egui::pos2(playhead_x + 6.0, lane.top() - 2.0),
                            egui::pos2(playhead_x + 6.0, lane.top() + 2.0),
                            egui::pos2(playhead_x, lane.top() + 9.0),
                            egui::pos2(playhead_x - 6.0, lane.top() + 2.0),
                        ],
                        playhead_color,
                        egui::Stroke::NONE,
                    ));
                }

                if can_seek && !handle_interaction {
                    let timeline_offset = self.timeline_offset;
                    let pointer_time = |position: egui::Pos2| {
                        let progress = ((position.x - lane.left()) / lane.width()).clamp(0.0, 1.0);
                        timeline_offset + progress as f64 * visible_duration
                    };

                    if response.dragged() {
                        self.seeking = true;
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                    }
                    if response.drag_stopped() {
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                        self.seeking = false;
                        if let Err(error) = self.playback.seek(self.playback_position) {
                            eprintln!("Blaze: {error}");
                        }
                    } else if response.clicked() {
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                            if let Err(error) = self.playback.seek(self.playback_position) {
                                eprintln!("Blaze: {error}");
                            }
                        }
                    }

                    // The ruler shares the waveform's time mapping and can be
                    // clicked or dragged to seek to the corresponding position.
                    if ruler_response.dragged() {
                        self.seeking = true;
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                    }
                    if ruler_response.drag_stopped() {
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                        self.seeking = false;
                        if let Err(error) = self.playback.seek(self.playback_position) {
                            eprintln!("Blaze: {error}");
                        }
                    } else if ruler_response.clicked() {
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                            if let Err(error) = self.playback.seek(self.playback_position) {
                                eprintln!("Blaze: {error}");
                            }
                        }
                    }
                }

                // A minimal horizontal scrollbar appears while the pointer is over
                // the timeline. Its thumb represents the visible time range.
                let scrollbar_track = egui::Rect::from_min_max(
                    egui::pos2(lane.left(), available.bottom() - 6.0),
                    egui::pos2(lane.right(), available.bottom() - 2.0),
                );
                let pointer_over_timeline = ui.input(|input| {
                    input.pointer.hover_pos().is_some_and(|position| available.contains(position))
                });
                let scrollbar_response = ui.interact(
                    scrollbar_track.expand2(egui::vec2(0.0, 4.0)),
                    ui.scope_id().with("timeline-horizontal-scrollbar"),
                    egui::Sense::click_and_drag(),
                );
                if pointer_over_timeline || scrollbar_response.hovered() || scrollbar_response.dragged() {
                    let thumb_width = (scrollbar_track.width()
                        * (visible_duration / duration).clamp(0.0, 1.0) as f32)
                        .clamp(18.0, scrollbar_track.width());
                    let thumb_left = scrollbar_track.left()
                        + (self.timeline_offset / duration).clamp(0.0, 1.0) as f32
                            * (scrollbar_track.width() - thumb_width);
                    let thumb = egui::Rect::from_min_max(
                        egui::pos2(thumb_left, scrollbar_track.top()),
                        egui::pos2(thumb_left + thumb_width, scrollbar_track.bottom()),
                    );
                    if scrollbar_response.dragged() || scrollbar_response.clicked() {
                        if let Some(pointer) = scrollbar_response.interact_pointer_pos() {
                            let thumb_left = (pointer.x - thumb_width * 0.5 - scrollbar_track.left())
                                .clamp(0.0, scrollbar_track.width() - thumb_width);
                            self.timeline_offset = (thumb_left
                                / (scrollbar_track.width() - thumb_width).max(1.0)
                                * (duration - visible_duration).max(0.0) as f32) as f64;
                            self.timeline_offset = self.timeline_offset.clamp(
                                0.0,
                                (duration - visible_duration).max(0.0),
                            );
                        }
                    }
                    let painter = ui.painter();
                    painter.rect_filled(
                        scrollbar_track,
                        egui::CornerRadius::same(2),
                        egui::Color32::from_rgba_unmultiplied(105, 110, 116, 55),
                    );
                    painter.rect_filled(
                        thumb,
                        egui::CornerRadius::same(2),
                        if scrollbar_response.dragged() {
                            egui::Color32::from_rgb(255, 104, 79)
                        } else {
                            egui::Color32::from_rgb(190, 195, 201)
                        },
                    );
                }

                // Keep the panel's requested height after a resize.
                ui.allocate_space(ui.available_size());
            });
        let timeline_split_rect = timeline_panel.response.rect;

        egui::Panel::right("tools")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(10, 11, 12))
                    .stroke(egui::Stroke::NONE)
                    // Keep the Tools content inset fixed across Normal and Expert modes.
                    .inner_margin(egui::Margin::symmetric(28, 14)),
            )
            .show_separator_line(false)
            .resizable(false)
            .default_size(460.0)
            .exact_size(saved_tools_width)
            .show(ui, |ui| {
                let tools_panel_rect = ui.max_rect();
                let pointer_over_tools_panel = ui.input(|input| {
                    input.pointer.hover_pos().is_some_and(|position| tools_panel_rect.contains(position))
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let mut tabs = vec![ToolTab::ModeSelector];
                    if let Some(mode) = self.active_mode {
                        tabs.push(match mode {
                            EditingMode::Blaze => ToolTab::BlazeMode,
                            EditingMode::Precise => ToolTab::PreciseMode,
                        });
                    }
                    tabs.push(ToolTab::Export);
                    tabs.push(ToolTab::License);

                    for tab in tabs {
                        if ui.selectable_label(self.active_tool_tab == tab, tab.label()).clicked() {
                            self.active_tool_tab = tab;
                        }
                    }
                });
                ui.separator();

                let tools_content_width = ui.available_width();
                {
                    let scroll = &mut ui.style_mut().spacing.scroll;
                    let opacity = if pointer_over_tools_panel { 0.72 } else { 0.0 };
                    scroll.floating = false;
                    scroll.bar_width = 5.0;
                    scroll.bar_inner_margin = 4.0;
                    scroll.foreground_color = false;
                    scroll.dormant_background_opacity = 0.0;
                    scroll.active_background_opacity = 0.0;
                    scroll.interact_background_opacity = 0.0;
                    scroll.dormant_handle_opacity = opacity;
                    scroll.active_handle_opacity = opacity;
                    scroll.interact_handle_opacity = opacity;
                }
                egui::ScrollArea::vertical()
                    .id_salt("tools-content-scroll")
                    // Expert Mode adds wider controls. Cap the scroll area's outer
                    // frame at the panel width so content cannot resize the panel.
                    .max_width(tools_content_width)
                    .auto_shrink([true, false])
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        ui.set_width(ui.available_width());

                        match self.active_tool_tab {
                    ToolTab::ModeSelector => {
                        ui.heading("Choose an editing mode");
                        ui.add_space(18.0);
                        let mode_card_height =
                            ((ui.available_height() - 92.0) * 0.5).clamp(116.0, 176.0);

                        let blaze_ready = self.selected_path.is_none()
                            || (!self.keyframes_running
                                && self.keyframe_error.is_none()
                                && !self.keyframes.is_empty());

                        let blaze_response = mode_selection_card(
                            ui,
                            "Blaze Mode",
                            "Make quick cuts while keeping the original video quality.",
                            ModeCardIcon::Blaze,
                            mode_card_height,
                            self.active_mode == Some(EditingMode::Blaze),
                            blaze_ready,
                        );
                        if blaze_response.clicked() && blaze_ready {
                            let keyframes = self.keyframes.clone();
                            self.edit_regions = self.edit_regions.iter().filter_map(|region| {
                                let start = nearest_keyframe(&keyframes, region.start)?;
                                let end = nearest_boundary_keyframe(&keyframes, region.end, start, false)?;
                                (end > start).then_some(EditRegion { start, end })
                            }).collect();
                            self.pending_region_start = self.pending_region_start
                                .and_then(|start| nearest_keyframe(&keyframes, start));
                            self.active_mode = Some(EditingMode::Blaze);
                            self.active_tool_tab = ToolTab::BlazeMode;
                        }

                        if self.keyframes_running && self.selected_path.is_some() {
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.small("Analysing GOP / keyframe structure…");
                            });
                        } else if let Some(error) = &self.keyframe_error {
                            ui.add_space(5.0);
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                format!("Keyframe analysis failed: {error}"),
                            );
                        } else if !self.keyframes.is_empty() {
                            ui.add_space(5.0);
                            ui.small(format!("{} video keyframes available.", self.keyframes.len()));
                        }

                        ui.add_space(18.0);
                        let precise_response = mode_selection_card(
                            ui,
                            "Precision Mode",
                            "Choose exactly where every cut begins and ends.",
                            ModeCardIcon::Precision,
                            mode_card_height,
                            self.active_mode == Some(EditingMode::Precise),
                            true,
                        );
                        if precise_response.clicked() {
                            self.active_mode = Some(EditingMode::Precise);
                            self.active_tool_tab = ToolTab::PreciseMode;
                        }
                    }
                    ToolTab::BlazeMode | ToolTab::PreciseMode => {
                        let expert_mode_changed =
                            ui.checkbox(&mut self.expert_mode, "Expert Mode").changed();
                        if expert_mode_changed && self.expert_mode {
                            // Initialize Expert Mode from the currently selected presets,
                            // rather than exposing unrelated hard-coded defaults.
                            let level = match self.detection_level {
                                0 => DetectionLevel::Comfortable,
                                1 => DetectionLevel::Optimal,
                                2 => DetectionLevel::Default,
                                3 => DetectionLevel::Tight,
                                _ => DetectionLevel::Aggressive,
                            };
                            self.custom_threshold_db = level.threshold_below_baseline_db();
                            let padding = match self.padding_level {
                                0 => PaddingPreset::Comfortable.milliseconds(),
                                2 => PaddingPreset::Tight.milliseconds(),
                                _ => PaddingPreset::Normal.milliseconds(),
                            };
                            self.custom_padding_before_ms = padding;
                            self.custom_padding_after_ms = padding;
                        }

                        ui.add_space(12.0);

                        if self.expert_mode {
                            let reset_threshold = match self.detection_level {
                                0 => DetectionLevel::Comfortable,
                                1 => DetectionLevel::Optimal,
                                2 => DetectionLevel::Default,
                                3 => DetectionLevel::Tight,
                                _ => DetectionLevel::Aggressive,
                            }
                            .threshold_below_baseline_db();
                            slider_control(
                                ui,
                                "Detection Threshold",
                                &mut self.custom_threshold_db,
                                Some("Offset below the adaptive programme baseline"),
                                None,
                                |ui, value| {
                                    numeric_field_f64(
                                        ui, value, 3.0, 60.0, 1.0, 1, " dB", 82.0,
                                        "Click to type · Arrow Up/Arrow Down changes by 1 dB",
                                    );
                                },
                                |ui, value| {
                                    let response = ui
                                        .add_sized(
                                            [ui.available_width(), 30.0],
                                            egui::Slider::new(value, 3.0..=60.0)
                                                .suffix(" dB")
                                                .fixed_decimals(1)
                                                .show_value(false)
                                                .trailing_fill(true),
                                        )
                                        .on_hover_text("Drag to adjust · double-click the handle to reset");
                                    if slider_handle_double_clicked(ui, &response, *value, 3.0, 60.0) {
                                        *value = reset_threshold;
                                    } else if let Some(direction) = take_wheel_step(ui, &response) {
                                        *value = (*value + direction).clamp(3.0, 60.0);
                                    }
                                },
                            );

                            ui.add_space(14.0);
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgb(23, 25, 28))
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    egui::Color32::from_rgb(43, 46, 50),
                                ))
                                .corner_radius(egui::CornerRadius::same(7))
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Peak Ceiling")
                                                .strong()
                                                .size(13.0),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                numeric_field_f64(
                                                    ui,
                                                    &mut self.custom_peak_ceiling_dbfs,
                                                    -90.0,
                                                    0.0,
                                                    1.0,
                                                    1,
                                                    " dBFS",
                                                    96.0,
                                                    "Click to type · Arrow Up/Arrow Down changes by 1 dB",
                                                );
                                            },
                                        );
                                    });
                                    ui.add_space(2.0);
                                    ui.checkbox(
                                        &mut self.custom_peak_gate_enabled,
                                        "Enable peak ceiling",
                                    );
                                    if self.custom_peak_gate_enabled {
                                        ui.add_space(3.0);
                                        ui.scope(|ui| {
                                            ui.spacing_mut().slider_width =
                                                (ui.available_width() - 12.0).max(1.0);
                                            ui.spacing_mut().slider_rail_height = 5.0;
                                            let visuals = ui.visuals_mut();
                                            visuals.handle_shape = egui::style::HandleShape::Circle;
                                            visuals.slider_trailing_fill = true;
                                            visuals.selection.bg_fill =
                                                egui::Color32::from_rgb(255, 104, 79);
                                            visuals.widgets.inactive.bg_fill =
                                                egui::Color32::from_rgb(55, 59, 64);
                                            visuals.widgets.inactive.fg_stroke =
                                                egui::Stroke::new(
                                                    1.5,
                                                    egui::Color32::from_rgb(210, 213, 216),
                                                );
                                            visuals.widgets.hovered.bg_fill =
                                                egui::Color32::from_rgb(67, 71, 77);
                                            visuals.widgets.hovered.fg_stroke =
                                                egui::Stroke::new(
                                                    1.7,
                                                    egui::Color32::from_rgb(255, 153, 132),
                                                );
                                            visuals.widgets.active.bg_fill =
                                                egui::Color32::from_rgb(255, 104, 79);
                                            visuals.widgets.active.fg_stroke =
                                                egui::Stroke::new(1.7, egui::Color32::WHITE);
                                            let response = ui
                                                .add_sized(
                                                    [ui.available_width(), 30.0],
                                                    egui::Slider::new(
                                                        &mut self.custom_peak_ceiling_dbfs,
                                                        -90.0..=0.0,
                                                    )
                                                    .suffix(" dBFS")
                                                    .fixed_decimals(1)
                                                    .show_value(false)
                                                    .trailing_fill(true),
                                                )
                                                .on_hover_text("Double-click the handle to reset");
                                            if slider_handle_double_clicked(
                                                ui,
                                                &response,
                                                self.custom_peak_ceiling_dbfs,
                                                -90.0,
                                                0.0,
                                            ) {
                                                self.custom_peak_ceiling_dbfs = -35.0;
                                            } else if let Some(direction) = take_wheel_step(ui, &response) {
                                                self.custom_peak_ceiling_dbfs =
                                                    (self.custom_peak_ceiling_dbfs + direction).clamp(-90.0, 0.0);
                                            }
                                        });
                                    }
                                });

                            ui.add_space(18.0);
                            slider_control(
                                ui,
                                "Minimum Silence Duration",
                                &mut self.minimum_silence_ms,
                                Some("Shortest pause that qualifies as silence. Default is 300 ms."),
                                None,
                                |ui, value| {
                                    numeric_field_u32(
                                        ui, value, 10, 10_000, 10.0, "", " ms", 86.0,
                                        "Click to type · Arrow Up/Arrow Down changes by 10 ms",
                                    );
                                },
                                |ui, value| {
                                    let response = ui.add_sized(
                                        [ui.available_width(), 30.0],
                                        egui::Slider::new(value, 10..=10_000)
                                            .suffix(" ms")
                                            .show_value(false)
                                            .trailing_fill(true),
                                    )
                                    .on_hover_text("Drag to adjust · double-click the handle to reset");
                                    if slider_handle_double_clicked(
                                        ui, &response, *value as f64, 10.0, 10_000.0,
                                    ) {
                                        *value = 300;
                                    } else if let Some(direction) = take_wheel_step(ui, &response) {
                                        let next = *value as f64 + direction * 10.0;
                                        *value = next.round().clamp(10.0, 10_000.0) as u32;
                                    }
                                },
                            );
                            ui.add_space(14.0);
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgb(23, 25, 28))
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 50)))
                                .corner_radius(egui::CornerRadius::same(7))
                                .inner_margin(egui::Margin::symmetric(12, 10))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("Padding").strong().size(13.0));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let link_icon = if self.padding_linked {
                                                egui::Image::new(egui::include_image!("../assets/link.svg"))
                                                    .tint(egui::Color32::from_rgb(255, 132, 108))
                                            } else {
                                                egui::Image::new(egui::include_image!("../assets/unlink.svg"))
                                                    .tint(egui::Color32::from_rgb(176, 181, 186))
                                            };
                                            let link = ui.add(
                                                egui::Button::image(link_icon.fit_to_exact_size(egui::vec2(16.0, 16.0)))
                                                    .min_size(egui::vec2(34.0, 28.0))
                                                    .selected(self.padding_linked),
                                            ).on_hover_text(if self.padding_linked {
                                                "Unlink left and right padding"
                                            } else {
                                                "Link left and right padding to one master control"
                                            });
                                            if link.clicked() {
                                                self.padding_linked = !self.padding_linked;
                                                if self.padding_linked {
                                                    self.custom_padding_after_ms = self.custom_padding_before_ms;
                                                }
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new("Preserve audio around detected silence")
                                            .size(11.5)
                                            .color(ui.visuals().weak_text_color()),
                                    );
                                    ui.add_space(10.0);

                                    if self.padding_linked {
                                        padding_control_row(
                                            ui,
                                            "L/R",
                                            &mut self.custom_padding_before_ms,
                                            "Linked left and right padding · Arrow Up/Arrow Down changes by 10 ms",
                                        );
                                        self.custom_padding_after_ms = self.custom_padding_before_ms;
                                    } else {
                                        padding_control_row(
                                            ui,
                                            "L",
                                            &mut self.custom_padding_before_ms,
                                            "Left padding · Arrow Up/Arrow Down changes by 10 ms",
                                        );
                                        ui.add_space(10.0);
                                        padding_control_row(
                                            ui,
                                            "R",
                                            &mut self.custom_padding_after_ms,
                                            "Right padding · Arrow Up/Arrow Down changes by 10 ms",
                                        );
                                    }
                                });
                            ui.add_space(14.0);
                        } else {
                            const DETECTION_LABELS: [&str; 5] = [
                                "Comfortable",
                                "Optimal",
                                "Default",
                                "Tight",
                                "Aggressive",
                            ];
                            let selected_detection_index = self.detection_level;
                            slider_control(
                                ui,
                                "Detection Level",
                                &mut self.detection_level,
                                Some("Set how sensitively Blaze identifies quiet sections."),
                                Some((&DETECTION_LABELS, selected_detection_index)),
                                |ui, value| {
                                    preset_value_field(
                                        ui,
                                        value,
                                        &DETECTION_DB_VALUES,
                                        " dB",
                                        82.0,
                                        "Type a dB value to choose the nearest preset · Arrow Up/Arrow Down moves one preset",
                                    );
                                },
                                |ui, value| {
                                    let response = ui
                                        .add_sized(
                                            [ui.available_width(), 32.0],
                                            egui::Slider::new(value, 0..=4)
                                                .step_by(1.0)
                                                .show_value(false)
                                                .trailing_fill(true),
                                        )
                                        .on_hover_text("Drag to choose a preset · double-click to reset");
                                    if slider_handle_double_clicked(
                                        ui, &response, *value as f64, 0.0, 4.0,
                                    ) {
                                        *value = 2;
                                    } else if let Some(direction) = take_wheel_step(ui, &response) {
                                        let next = *value as f64 + direction;
                                        *value = next.round().clamp(0.0, 4.0) as usize;
                                    }
                                },
                            );
                            ui.add_space(18.0);
                            slider_control(
                                ui,
                                "Minimum Silence Duration",
                                &mut self.minimum_silence_ms,
                                Some("Shortest pause that qualifies as silence. Default is 300 ms."),
                                None,
                                |ui, value| {
                                    numeric_field_u32(
                                        ui, value, 10, 10_000, 10.0, "", " ms", 86.0,
                                        "Click to type · Arrow Up/Arrow Down changes by 10 ms",
                                    );
                                },
                                |ui, value| {
                                    let response = ui
                                        .add_sized(
                                            [ui.available_width(), 30.0],
                                            egui::Slider::new(value, 10..=10_000)
                                                .suffix(" ms")
                                                .show_value(false)
                                                .trailing_fill(true),
                                        )
                                        .on_hover_text("Drag to adjust · double-click the handle to reset");
                                    if slider_handle_double_clicked(
                                        ui, &response, *value as f64, 10.0, 10_000.0,
                                    ) {
                                        *value = 300;
                                    } else if let Some(direction) = take_wheel_step(ui, &response) {
                                        let next = *value as f64 + direction * 10.0;
                                        *value = next.round().clamp(10.0, 10_000.0) as u32;
                                    }
                                },
                            );
                            ui.add_space(18.0);
                            const PADDING_LABELS: [&str; 3] = [
                                "Comfortable",
                                "Normal",
                                "Tight",
                            ];
                            let selected_padding_index = self.padding_level;
                            slider_control(
                                ui,
                                "Padding",
                                &mut self.padding_level,
                                Some("Choose how much audio to preserve around each detected pause."),
                                Some((&PADDING_LABELS, selected_padding_index)),
                                |ui, value| {
                                    preset_value_field(
                                        ui,
                                        value,
                                        &PADDING_MS_VALUES,
                                        " ms",
                                        86.0,
                                        "Type a millisecond value to choose the nearest preset · Arrow Up/Arrow Down moves one preset",
                                    );
                                },
                                |ui, value| {
                                    let response = ui
                                        .add_sized(
                                            [ui.available_width(), 32.0],
                                            egui::Slider::new(value, 0..=2)
                                                .step_by(1.0)
                                                .show_value(false)
                                                .trailing_fill(true),
                                        )
                                        .on_hover_text("Drag to choose a preset · double-click to reset");
                                    if slider_handle_double_clicked(
                                        ui, &response, *value as f64, 0.0, 2.0,
                                    ) {
                                        *value = 1;
                                    } else if let Some(direction) = take_wheel_step(ui, &response) {
                                        let next = *value as f64 + direction;
                                        *value = next.round().clamp(0.0, 2.0) as usize;
                                    }
                                },
                            );
                        }

                        ui.add_space(20.0);
                        let blaze_ready = self.active_mode != Some(EditingMode::Blaze)
                            || (!self.keyframes_running
                                && self.keyframe_error.is_none()
                                && !self.keyframes.is_empty());
                        let can_detect = self.selected_path.is_some()
                            && self.media_info.as_ref().is_some_and(|info| info.has_audio)
                            && !self.silence_running
                            && blaze_ready;
                        let detect_label = if self.silence_running {
                            "Analysing Audio…"
                        } else {
                            "Detect Silence"
                        };
                        let detect_response = ui.scope(|ui| {
                            let visuals = &mut ui.style_mut().visuals.widgets;
                            let button_text = egui::Color32::from_rgb(255, 247, 243);
                            let border = egui::Color32::from_rgb(190, 91, 67);
                            visuals.inactive.bg_fill = egui::Color32::from_rgb(112, 48, 36);
                            visuals.inactive.bg_stroke =
                                egui::Stroke::new(1.0, border);
                            visuals.inactive.fg_stroke =
                                egui::Stroke::new(1.0, button_text);
                            visuals.hovered.bg_fill = egui::Color32::from_rgb(143, 61, 44);
                            visuals.hovered.bg_stroke =
                                egui::Stroke::new(1.2, egui::Color32::from_rgb(230, 119, 88));
                            visuals.hovered.fg_stroke =
                                egui::Stroke::new(1.0, button_text);
                            visuals.active.bg_fill = egui::Color32::from_rgb(88, 38, 30);
                            visuals.active.bg_stroke =
                                egui::Stroke::new(1.0, border);
                            visuals.active.fg_stroke =
                                egui::Stroke::new(1.0, button_text);

                            ui.add_enabled_ui(can_detect, |ui| {
                                ui.add_sized(
                                    [ui.available_width(), 40.0],
                                    egui::Button::new(
                                        egui::RichText::new(detect_label)
                                            .size(14.0)
                                            .strong(),
                                    )
                                    .corner_radius(egui::CornerRadius::same(7)),
                                )
                                .on_hover_text("Scan the video's audio and mark detected silent sections")
                            }).inner
                        }).inner;
                        if detect_response.clicked() {
                            self.start_silence_detection(ui.ctx());
                        }
                        if self.silence_running {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.small("Analysing audio. A rescan replaces existing regions.");
                            });
                        }
                    }
                    ToolTab::Export => {
                        ui.heading("Export");
                        ui.add_space(12.0);

                        ui.strong("Video Export");
                        ui.add_space(6.0);

                        let blaze_reason = if self.selected_path.is_none() {
                            Some("Import a video to begin.")
                        } else if self.loading || self.media_info.is_none() {
                            Some("Wait for video analysis to finish.")
                        } else if self.active_mode != Some(EditingMode::Blaze) {
                            Some("Requires Blaze Mode. Switch to Blaze Mode to enable this export.")
                        } else if self.keyframes_running {
                            Some("Waiting for keyframe analysis to finish.")
                        } else if self.keyframe_error.is_some() || self.keyframes.is_empty() {
                            Some("Blaze Export requires usable video keyframes.")
                        } else if self.edit_regions.is_empty() {
                            Some("Create at least one cut region before exporting.")
                        } else if !self.can_export() {
                            Some("Free export limit reached. Activate Blaze to continue exporting.")
                        } else {
                            None
                        };

                        let blaze_eligible = blaze_reason.is_none() && !self.export_running;
                        let blaze_clicked = ui.add_enabled(
                            blaze_eligible,
                            egui::Button::new(egui::RichText::new(
                                if self.export_running { "Blaze Export — Exporting…" } else { "Blaze Export" }
                            ).strong())
                                .min_size(egui::vec2(ui.available_width(), 38.0)),
                        ).clicked();

                        if let Some(reason) = blaze_reason {
                            ui.label(egui::RichText::new(reason).small().color(ui.visuals().weak_text_color()));
                        }
                        if blaze_clicked {
                            if let Some(source) = self.selected_path.clone() {
                                let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("mkv").to_string();
                                let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
                                let filename = format!("{stem}_Blaze.{extension}");
                                if let Some(output) = FileDialog::new()
                                    .add_filter("Video", &[extension.as_str()])
                                    .set_file_name(&filename)
                                    .save_file()
                                {
                                    self.start_blaze_export(output, ui.ctx());
                                }
                            }
                        }

                        ui.add_space(8.0);
                        ui.add_enabled(
                            false,
                            egui::Button::new("Precision Export")
                                .min_size(egui::vec2(ui.available_width(), 38.0)),
                        );
                        ui.label(
                            egui::RichText::new("Coming Soon — re-encoded export for Precise Mode.")
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );

                        ui.add_space(22.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.strong("Timestamp Export");
                        ui.label(
                            egui::RichText::new("Export the current edit as an editable NLE timeline.")
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.add_space(8.0);

                        let timestamp_available = self.selected_path.is_some()
                            && self.media_info.is_some()
                            && !self.loading
                            && !self.export_running
                            && self.can_export();
                        let timestamp_formats = [
                            ("Adobe Premiere Pro (FCP7 XML)", "xml", "Premiere_FCP7_XML", InterchangeFormat::PremiereXml),
                            ("DaVinci Resolve (FCP7 XML)", "xml", "DaVinci_Resolve_FCP7_XML", InterchangeFormat::DavinciResolve),
                            ("Final Cut Pro XML (1.14)", "fcpxml", "Final_Cut_Pro", InterchangeFormat::Fcpxml),
                            ("Edit Decision List (CMX 3600)", "edl", "EDL", InterchangeFormat::Edl),
                            ("OpenTimelineIO (.otio)", "otio", "OpenTimelineIO", InterchangeFormat::OpenTimelineIo),
                        ];

                        for (label, extension, suffix, format) in timestamp_formats {
                            let clicked = ui.add_enabled(
                                timestamp_available,
                                egui::Button::new(label)
                                    .min_size(egui::vec2(ui.available_width(), 34.0)),
                            ).clicked();
                            if clicked {
                                if let Some(source) = self.selected_path.clone() {
                                    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
                                    let filename = format!("{stem}_{suffix}.{extension}");
                                    let output = FileDialog::new()
                                        .add_filter("Timeline interchange", &[extension])
                                        .set_file_name(&filename)
                                        .save_file();
                                    if let Some(output) = output {
                                        self.start_interchange_export(format, output, ui.ctx());
                                    }
                                }
                            }
                        }

                        if !self.can_export() {
                            ui.label(egui::RichText::new("Free export limit reached. Activate Blaze to continue exporting.")
                                .small().color(egui::Color32::from_rgb(255, 132, 108)));
                        } else if !timestamp_available {
                            ui.label(egui::RichText::new("Import a video to enable timestamp exports.")
                                .small().color(ui.visuals().weak_text_color()));
                        }

                        if self.export_running {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| { ui.spinner(); ui.small("Preparing export…"); });
                        }
                        if let Some((success, message)) = &self.export_status {
                            ui.add_space(8.0);
                            let color = if *success { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_RED };
                            ui.colored_label(color, message);
                        }
                    }
                    ToolTab::License => {
                        ui.heading("License");
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("Check your activation status and manage access to Blaze.")
                            .size(12.0).color(ui.visuals().weak_text_color()));
                        ui.add_space(16.0);
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(23, 25, 28))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 50)))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("LICENSE STATUS").size(10.5).strong()
                                        .color(egui::Color32::from_rgb(255, 132, 108)));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let (status, color) = if self.license_activated {
                                            let label = match self.license_plan {
                                                Some(LicensePlan::Monthly) => "MONTHLY",
                                                Some(LicensePlan::Permanent) => "PERMANENT",
                                                None => "ACTIVATED",
                                            };
                                            (label, egui::Color32::LIGHT_GREEN)
                                        } else {
                                            ("FREE", ui.visuals().weak_text_color())
                                        };
                                        ui.label(egui::RichText::new(status).size(11.0).strong().color(color));
                                    });
                                });
                                ui.add_space(8.0);
                                if self.license_activated {
                                    let (plan_title, plan_description) = match self.license_plan {
                                        Some(LicensePlan::Monthly) => (
                                            "Blaze Monthly",
                                            self.license_valid_until.as_deref()
                                                .map(|date| format!("Access remains active through {date} UTC."))
                                                .unwrap_or_else(|| "Your monthly subscription is active.".to_owned())
                                        ),
                                        Some(LicensePlan::Permanent) => (
                                            "Blaze Permanent",
                                            "Your permanent licence is active. No subscription expiry.".to_owned()
                                        ),
                                        None => ("Blaze Activated", "Your licence is active.".to_owned()),
                                    };
                                    ui.label(egui::RichText::new(plan_title).size(17.0).strong());
                                    ui.label(egui::RichText::new(plan_description)
                                        .size(12.0).color(ui.visuals().weak_text_color()));
                                    ui.add_space(14.0);
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgb(19, 21, 24))
                                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 50)))
                                        .corner_radius(egui::CornerRadius::same(7))
                                        .inner_margin(egui::Margin::same(12))
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new("MOVING TO ANOTHER DEVICE?")
                                                .size(10.5).strong()
                                                .color(egui::Color32::from_rgb(255, 132, 108)));
                                            ui.add_space(6.0);
                                            ui.label(egui::RichText::new(
                                                "If you reinstall Blaze or switch to a different device or operating system, email us to request a license transfer."
                                            ).size(12.0).color(ui.visuals().weak_text_color()));
                                            ui.add_space(7.0);
                                            copy_support_email(ui);
                                        });
                                } else {
                                    ui.label(egui::RichText::new("Free version").size(17.0).strong());
                                    let remaining = FREE_EXPORT_LIMIT.saturating_sub(self.successful_exports);
                                    ui.label(egui::RichText::new(format!(
                                        "{} of {} free exports remaining.",
                                        remaining, FREE_EXPORT_LIMIT
                                    )).size(12.0).color(ui.visuals().weak_text_color()));
                                    ui.add_space(10.0);
                                    let get_key_clicked = ui.scope(|ui| {
                                        let widgets = &mut ui.style_mut().visuals.widgets;
                                        let button_text = egui::Color32::from_rgb(255, 247, 243);
                                        widgets.inactive.bg_fill = egui::Color32::from_rgb(255, 104, 79);
                                        widgets.inactive.bg_stroke = egui::Stroke::new(
                                            1.0, egui::Color32::from_rgb(255, 104, 79)
                                        );
                                        widgets.inactive.fg_stroke = egui::Stroke::new(1.0, button_text);
                                        widgets.hovered.bg_fill = egui::Color32::from_rgb(255, 143, 119);
                                        widgets.hovered.bg_stroke = egui::Stroke::new(
                                            1.0, egui::Color32::from_rgb(255, 143, 119)
                                        );
                                        widgets.hovered.fg_stroke = egui::Stroke::new(1.0, button_text);
                                        widgets.active.bg_fill = egui::Color32::from_rgb(190, 72, 54);
                                        widgets.active.bg_stroke = egui::Stroke::new(
                                            1.0, egui::Color32::from_rgb(190, 72, 54)
                                        );
                                        widgets.active.fg_stroke = egui::Stroke::new(1.0, button_text);
                                        ui.add_sized(
                                            [ui.available_width(), 36.0],
                                            egui::Button::new(
                                                egui::RichText::new("Get Activation Key!").strong()
                                            ).corner_radius(egui::CornerRadius::same(7))
                                        ).clicked()
                                    }).inner;
                                    if get_key_clicked {
                                        match open_external_url(
                                            "https://ankushsarkar369.github.io/blaze.github.io/"
                                        ) {
                                            Ok(()) => self.license_notice = None,
                                            Err(error) => {
                                                self.license_notice = Some(
                                                    format!("Couldn't open the website: {error}")
                                                );
                                            }
                                        }
                                    }
                                }
                            });

                        if !self.license_activated {
                            ui.add_space(14.0);
                            ui.label(egui::RichText::new("Enter your license key")
                                .size(12.0).color(ui.visuals().weak_text_color()));
                            ui.add_space(6.0);
                            ui.add(egui::TextEdit::singleline(&mut self.license_key_input)
                                .hint_text("Paste your license key")
                                .desired_width(f32::INFINITY));
                            ui.add_space(8.0);
                            let activate_width = ui.available_width();
                            let activate_clicked = ui.horizontal_centered(|ui| {
                                ui.add_enabled(
                                    !self.license_key_input.trim().is_empty()
                                        && !self.license_verifying
                                        && !self.license_activated,
                                    egui::Button::new(
                                        egui::RichText::new(
                                            if self.license_verifying { "Checking…" } else { "Activate Blaze!" }
                                        ).strong()
                                    ).min_size(egui::vec2(activate_width, 36.0)),
                                ).clicked()
                            }).inner;
                            if activate_clicked {
                                self.activate_license_key(ui.ctx());
                            }
                        }
                        if let Some(message) = &self.license_notice {
                            ui.add_space(8.0);
                            let color = if self.license_activated {
                                egui::Color32::LIGHT_GREEN
                            } else {
                                egui::Color32::from_rgb(235, 183, 113)
                            };
                            ui.colored_label(color, message);
                        }
                    }
                    }
                    });

                // Fill the remaining panel area after the scroll viewport.
                ui.allocate_space(ui.available_size());
            });
        // Keep the persisted panel geometry authoritative after content layout.
        // Expert Mode has wider intrinsic controls; without this write-back,
        // egui can persist their expanded minimum width and shift the splitter.
        let tools_panel_rect = egui::Rect::from_min_max(
            egui::pos2(root_rect.right() - saved_tools_width, root_rect.top()),
            egui::pos2(root_rect.right(), root_rect.bottom()),
        );
        ui.ctx().data_mut(|data| {
            data.insert_persisted(
                tools_panel_id,
                egui::containers::panel::PanelState {
                    outer_rect: egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(saved_tools_width, root_rect.height()),
                    ),
                },
            );
        });
        let tools_split_rect = tools_panel_rect;

        egui::CentralPanel::default()
            // Preview and timeline share one continuous workspace surface.
            // The timeline splitter is the only visual division between them.
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(10, 11, 12))
                    .stroke(egui::Stroke::NONE),
            )
            .show(ui, |ui| {
                // Keep only layout padding around the video canvas; do not
                // wrap the preview and transport in another visible card.
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        let available = ui.available_rect_before_wrap();
                        // Keep the preview and transport aligned to the fixed Tools splitter.
                        // The Tools content can otherwise make CentralPanel report a wider
                        // rect in Expert Mode, shifting the preview by the scrollbar width.
                        let available = egui::Rect::from_min_max(
                            available.min,
                            egui::pos2(
                                available.max.x.min(tools_split_rect.left() - 10.0),
                                available.max.y,
                            ),
                        );
                        let transport_height = 46.0_f32.min(available.height());
                        let preview_rect = egui::Rect::from_min_max(
                            available.min,
                            egui::pos2(available.max.x, available.max.y - transport_height),
                        );
                        let transport_rect = egui::Rect::from_min_max(
                            egui::pos2(available.min.x, preview_rect.max.y),
                            available.max,
                        );

                        ui.scope_builder(egui::UiBuilder::new().max_rect(preview_rect), |ui| {
                            let preview_rect = ui.max_rect();
                            ui.painter().rect_filled(
                                preview_rect,
                                egui::CornerRadius::same(3),
                                egui::Color32::from_rgb(10, 11, 12),
                            );

                            if self.selected_path.is_some() {
                                self.playback.paint(ui.painter(), preview_rect);
                            } else {
                                let button_rect = egui::Rect::from_center_size(
                                    preview_rect.center(),
                                    egui::vec2(156.0, 42.0),
                                );
                                if ui
                                    .put(button_rect, egui::Button::new("Import video"))
                                    .clicked()
                                {
                                    self.open_video(ui.ctx());
                                }
                            }
                        });

                        ui.scope_builder(egui::UiBuilder::new().max_rect(transport_rect), |ui| {
                            let transport_surface = ui.max_rect();
                            ui.painter().line_segment(
                                [
                                    transport_surface.left_top(),
                                    transport_surface.right_top(),
                                ],
                                egui::Stroke::new(
                                    1.0,
                                    egui::Color32::from_rgb(43, 46, 50),
                                ),
                            );

                            if self.selected_path.is_some() {
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(8.0);

                                        let paused = self.playback.is_paused();
                                        let icon = if paused { PlayerIcon::Play } else { PlayerIcon::Pause };
                                        let tooltip = if paused { "Play" } else { "Pause" };

                                        if allocated_icon_button(
                                            ui,
                                            "transport-play-pause",
                                            egui::vec2(34.0, 30.0),
                                            icon,
                                            tooltip,
                                        )
                                        .clicked()
                                        {
                                            if let Err(error) = self.playback.toggle_pause() {
                                                eprintln!("Blaze: {error}");
                                            }
                                        }

                                        ui.add_space(4.0);

                                        let duration = self.media_duration();
                                        self.playback_position = self.playback_position.clamp(0.0, duration);

                                        let time_label = format!(
                                            "{} / {}",
                                            format_time_millis(self.playback_position),
                                            format_time_millis(duration)
                                        );
                                        let time_font = egui::FontId::proportional(13.5);
                                        let time_width = ui
                                            .painter()
                                            .layout_no_wrap(
                                                time_label.clone(),
                                                time_font.clone(),
                                                ui.visuals().text_color(),
                                            )
                                            .size()
                                            .x
                                            + 2.0;
                                        let skip_button_width = 112.0;
                                        let slider_width = (ui.available_width()
                                            - time_width
                                            - skip_button_width
                                            - 34.0)
                                            .max(36.0);

                                        // Custom transport seekbar: a continuous orange fill,
                                        // dark remainder and a white circular handle.
                                        let (seek_id, seek_rect) =
                                            ui.allocate_space(egui::vec2(slider_width, 32.0));
                                        let seek_response = ui
                                            .interact(seek_rect, seek_id, egui::Sense::click_and_drag())
                                            .on_hover_text("Seek");
                                        let track_width = (seek_rect.width() - 18.0).max(1.0);
                                        let track = egui::Rect::from_center_size(
                                            seek_rect.center(),
                                            egui::vec2(track_width, 4.0),
                                        );

                                        let drag_stopped = seek_response.drag_stopped();
                                        let clicked = seek_response.clicked();
                                        let dragging = seek_response.dragged();

                                        if dragging || drag_stopped || clicked {
                                            if let Some(pointer) = seek_response.interact_pointer_pos() {
                                                let progress = ((pointer.x - track.left()) / track.width())
                                                    .clamp(0.0, 1.0) as f64;
                                                self.playback_position = progress * duration;

                                                // Keep the timeline viewport in sync with preview seeking.
                                                let visible_duration =
                                                    duration / self.timeline_zoom as f64;
                                                self.timeline_offset = (self.playback_position
                                                    - visible_duration * 0.5)
                                                    .clamp(0.0, (duration - visible_duration).max(0.0));
                                            }

                                            if drag_stopped || clicked {
                                                self.seeking = false;
                                                if let Err(error) = self.playback.seek(self.playback_position) {
                                                    eprintln!("Blaze: {error}");
                                                }
                                            } else {
                                                self.seeking = true;
                                            }
                                        }

                                        let progress =
                                            (self.playback_position / duration).clamp(0.0, 1.0) as f32;
                                        let thumb_x = track.left() + track.width() * progress;
                                        let track_center = track.center().y;
                                        let painter = ui.painter();
                                        painter.rect_filled(
                                            track,
                                            egui::CornerRadius::same(2),
                                            egui::Color32::from_rgb(67, 69, 72),
                                        );
                                        let filled_track = egui::Rect::from_min_max(
                                            track.min,
                                            egui::pos2(thumb_x, track.bottom()),
                                        );
                                        painter.rect_filled(
                                            filled_track,
                                            egui::CornerRadius::same(2),
                                            egui::Color32::from_rgb(255, 104, 79),
                                        );
                                        painter.circle_filled(
                                            egui::pos2(thumb_x, track_center),
                                            if seek_response.hovered() || seek_response.dragged() {
                                                8.0
                                            } else {
                                                7.0
                                            },
                                            egui::Color32::from_rgb(236, 239, 242),
                                        );

                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [time_width, 26.0],
                                            egui::Label::new(
                                                egui::RichText::new(time_label)
                                                    .font(time_font),
                                            ),
                                        );

                                        ui.add_space(8.0);
                                        let skip_label = if self.skip_cuts_enabled {
                                            "Skip Cuts: ON"
                                        } else {
                                            "Skip Cuts: OFF"
                                        };
                                        let skip_button = egui::Button::new(
                                            egui::RichText::new(skip_label)
                                                .font(egui::FontId::proportional(13.5)),
                                        )
                                        .fill(if self.skip_cuts_enabled {
                                            egui::Color32::from_rgb(38, 111, 73)
                                        } else {
                                            egui::Color32::from_rgb(126, 48, 44)
                                        });
                                        if ui
                                            .add_sized([skip_button_width, 32.0], skip_button)
                                            .on_hover_text(
                                                "Automatically skip selected regions during playback",
                                            )
                                            .clicked()
                                        {
                                            self.skip_cuts_enabled = !self.skip_cuts_enabled;
                                        }
                                        // Keep a deliberate gap between the button and the preview panel edge.
                                        ui.add_space(16.0);
                                    },
                                );
                            }
                        });
                    });
            });

        // Use explicit splitter hit-zones instead of egui Panel's built-in
        // resizers. The native panel stroke uses the panel's internal cross-axis
        // bounds, which can leave a stale-looking separator where the panels meet.
        // These hit-zones are calculated from the root viewport and never overlap:
        // the tools splitter ends before the timeline splitter begins.
        let upper_panel_rect = egui::Rect::from_min_max(
            root_rect.min,
            egui::pos2(root_rect.right(), timeline_split_rect.top()),
        );
        let tools_split_x = tools_split_rect.left();
        let timeline_split_y = timeline_split_rect.top();

        let tools_resize_rect = egui::Rect::from_min_max(
            egui::pos2(tools_split_x - 10.0, upper_panel_rect.top()),
            egui::pos2(
                tools_split_x + 10.0,
                (upper_panel_rect.bottom() - 10.0).max(upper_panel_rect.top()),
            ),
        );
        let tools_resize_response = ui
            .interact(
                tools_resize_rect,
                ui.scope_id().with("tools").with("__resize"),
                egui::Sense::click_and_drag(),
            )
            .on_hover_text("Drag to resize · Double-click to reset");

        if tools_resize_response.hovered() || tools_resize_response.dragged() {
            ui.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if tools_resize_response.double_clicked() {
            let width = self.media_info.as_ref()
                .map(|info| default_tools_width(root_rect, info))
                .unwrap_or(460.0)
                .clamp(tools_min_width, tools_max_width);
            let panel_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, root_rect.height()),
            );
            let panel_id = ui.scope_id().with("tools");
            ui.ctx().data_mut(|data| {
                data.insert_persisted(
                    panel_id,
                    egui::containers::panel::PanelState { outer_rect: panel_rect },
                );
            });
            ui.ctx().request_repaint();
        } else if tools_resize_response.dragged() || tools_resize_response.drag_stopped() {
            if let Some(pointer) = tools_resize_response.interact_pointer_pos() {
                let width = (root_rect.right() - pointer.x)
                    .clamp(tools_min_width, tools_max_width);
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, root_rect.height()),
                );
                let panel_id = ui.scope_id().with("tools");
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        panel_id,
                        egui::containers::panel::PanelState { outer_rect: panel_rect },
                    );
                });
                ui.ctx().request_repaint();
            }
        }

        let timeline_resize_rect = egui::Rect::from_min_max(
            egui::pos2(root_rect.left(), timeline_split_y - 10.0),
            egui::pos2(root_rect.right(), timeline_split_y + 10.0),
        ).intersect(root_rect);
        let timeline_resize_response = ui
            .interact(
                timeline_resize_rect,
                ui.scope_id().with("timeline").with("__resize"),
                egui::Sense::click_and_drag(),
            )
            .on_hover_text("Drag to resize · Double-click to reset");

        if timeline_resize_response.hovered() || timeline_resize_response.dragged() {
            ui.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }

        if timeline_resize_response.double_clicked() {
            let height = 220.0_f32.min(root_rect.height());
            let panel_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(root_rect.width(), height),
            );
            let panel_id = ui.scope_id().with("timeline");
            ui.ctx().data_mut(|data| {
                data.insert_persisted(
                    panel_id,
                    egui::containers::panel::PanelState { outer_rect: panel_rect },
                );
            });
            ui.ctx().request_repaint();
        } else if timeline_resize_response.dragged() || timeline_resize_response.drag_stopped() {
            if let Some(pointer) = timeline_resize_response.interact_pointer_pos() {
                let min_height = 140.0_f32.min(root_rect.height());
                let height = (root_rect.bottom() - pointer.y)
                    .clamp(min_height, root_rect.height().max(min_height));
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(root_rect.width(), height),
                );
                let panel_id = ui.scope_id().with("timeline");
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        panel_id,
                        egui::containers::panel::PanelState { outer_rect: panel_rect },
                    );
                });
                ui.ctx().request_repaint();
            }
        }

        // Draw splitter feedback only inside the region that the splitter owns.
        let handle_fill = egui::Color32::from_rgb(42, 45, 48);
        let handle_mark = egui::Color32::from_rgb(126, 130, 134);
        let tools_painter = ui.painter().with_clip_rect(upper_panel_rect);
        let tools_handle = egui::Rect::from_center_size(
            egui::pos2(tools_split_x, upper_panel_rect.center().y),
            egui::vec2(10.0, 62.0),
        ).intersect(upper_panel_rect);
        let tools_hovered = tools_resize_response.hovered() || tools_resize_response.dragged();
        let tools_handle_fill = if tools_hovered {
            egui::Color32::from_rgb(67, 71, 75)
        } else {
            handle_fill
        };
        tools_painter.rect_filled(
            tools_handle,
            egui::CornerRadius::same(5),
            tools_handle_fill,
        );
        for offset in [-8.0_f32, 0.0, 8.0] {
            tools_painter.circle_filled(
                egui::pos2(tools_handle.center().x, tools_handle.center().y + offset),
                1.25,
                handle_mark,
            );
        }
        let tools_stroke = if tools_resize_response.dragged() {
            ui.visuals().widgets.active.fg_stroke
        } else if tools_resize_response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            egui::Stroke::NONE
        };
        if tools_stroke.width > 0.0 {
            tools_painter.line_segment(
                [
                    egui::pos2(tools_split_x, upper_panel_rect.top()),
                    egui::pos2(tools_split_x, upper_panel_rect.bottom()),
                ],
                tools_stroke,
            );
        }

        let timeline_painter = ui.painter().with_clip_rect(root_rect);
        let timeline_handle = egui::Rect::from_center_size(
            egui::pos2(root_rect.center().x, timeline_split_y),
            egui::vec2(62.0, 10.0),
        ).intersect(root_rect);
        let timeline_hovered =
            timeline_resize_response.hovered() || timeline_resize_response.dragged();
        let timeline_handle_fill = if timeline_hovered {
            egui::Color32::from_rgb(67, 71, 75)
        } else {
            handle_fill
        };
        timeline_painter.rect_filled(
            timeline_handle,
            egui::CornerRadius::same(5),
            timeline_handle_fill,
        );
        for offset in [-8.0_f32, 0.0, 8.0] {
            timeline_painter.circle_filled(
                egui::pos2(timeline_handle.center().x + offset, timeline_handle.center().y),
                1.25,
                handle_mark,
            );
        }
        let timeline_stroke = if timeline_resize_response.dragged() {
            ui.visuals().widgets.active.fg_stroke
        } else if timeline_resize_response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke
        } else {
            egui::Stroke::NONE
        };
        if timeline_stroke.width > 0.0 {
            timeline_painter.line_segment(
                [
                    egui::pos2(root_rect.left(), timeline_split_y),
                    egui::pos2(root_rect.right(), timeline_split_y),
                ],
                timeline_stroke,
            );
        }

        if self.show_help {
            egui::Window::new("Quick Help")
                .collapsible(false)
                .resizable(true)
                .default_size(egui::vec2(940.0, 680.0))
                .min_size(egui::vec2(860.0, 580.0))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut self.show_help)
                .show(ui.ctx(), |ui| {
                    use egui::{FontFamily, FontId};

                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 58.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new("Keep The Moments That Matter")
                                    .font(FontId::new(20.0, FontFamily::Proportional))
                                    .strong(),
                            );
                            ui.add_space(3.0);
                            ui.label(
                                egui::RichText::new("Learn to mark edits, fine-tune silence detection, navigate the timeline and export your cuts.")
                                    .font(FontId::new(12.5, FontFamily::Proportional))
                                    .color(ui.visuals().weak_text_color()),
                            );
                        },
                    );
                    ui.add_space(10.0);
                    ui.separator();

                    let body_height = ui.available_height();
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(166.0, body_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.add_space(10.0);
                                for tab in [
                                    HelpTab::Guide,
                                    HelpTab::KeyboardMouse,
                                    HelpTab::Timeline,
                                    HelpTab::Blaze,
                                    HelpTab::Precision,
                                    HelpTab::Export,
                                    HelpTab::Activation,
                                ] {
                                    let selected = self.help_tab == tab;
                                    let width = (ui.available_width() - 18.0).max(100.0);
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(width, 34.0),
                                        egui::Sense::click(),
                                    );
                                    let painter = ui.painter();
                                    let fill = if selected {
                                        egui::Color32::from_rgb(35, 27, 25)
                                    } else if response.hovered() {
                                        egui::Color32::from_rgb(24, 26, 29)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    painter.rect_filled(rect, egui::CornerRadius::same(6), fill);
                                    if selected {
                                        painter.rect_filled(
                                            egui::Rect::from_min_max(
                                                egui::pos2(rect.left(), rect.top() + 7.0),
                                                egui::pos2(rect.left() + 3.0, rect.bottom() - 7.0),
                                            ),
                                            egui::CornerRadius::same(2),
                                            egui::Color32::from_rgb(255, 104, 79),
                                        );
                                    }
                                    painter.text(
                                        egui::pos2(rect.left() + 12.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        tab.label(),
                                        FontId::new(13.0, FontFamily::Proportional),
                                        if selected {
                                            egui::Color32::from_rgb(255, 143, 119)
                                        } else {
                                            ui.visuals().text_color()
                                        },
                                    );
                                    if response.clicked() {
                                        self.help_tab = tab;
                                    }
                                    ui.add_space(4.0);
                                }
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(8.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("QUICK TIP")
                                            .font(FontId::new(10.5, FontFamily::Proportional))
                                            .strong()
                                            .color(egui::Color32::from_rgb(255, 132, 108)),
                                    );
                                });
                                ui.label(
                                    egui::RichText::new("Hover over controls in Blaze for tooltips.")
                                        .font(FontId::new(12.0, FontFamily::Proportional))
                                        .color(ui.visuals().weak_text_color()),
                                );
                            },
                        );
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(10.0);

                        ui.vertical(|ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("quick-help-documentation")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add_space(4.0);
                                    match self.help_tab {
                                        HelpTab::Guide => {
                                            help_page_heading(ui, "Getting started", "The shortest path from an imported video to a reviewed edit.");
                                            help_card(ui, "1. Import a video", "Choose Import video in the preview area. Blaze probes the media, opens the preview and builds waveform data for timeline navigation. Wait for waveform generation to finish before editing.");
                                            help_card(ui, "2. Choose an editing mode", "Blaze Mode snaps cut boundaries to usable video keyframes and is designed for fast stream-copy exports. Precision Mode places boundaries at the current playback position without keyframe snapping.");
                                            help_card(ui, "3. Mark and review cuts", "Move the playhead to a start point and press S. Move to the end point and press D. Review the marked regions on the waveform; drag either boundary to adjust it.");
                                            help_card(ui, "4. Detect pauses automatically", "Open the silence detection controls, choose a detection level, minimum pause duration and padding, then select Detect Silence. Review the generated regions before exporting.");
                                            help_card(ui, "5. Export your result", "Use Blaze Export for a fast stream-copy output when available, or export an editable timeline for a supported NLE. Precision video export is not available yet.");
                                        }
                                        HelpTab::Blaze => {
                                            help_page_heading(ui, "Blaze Mode", "Fast, keyframe-aligned editing for stream-copy workflows.");
                                            help_card(ui, "How cuts work", "Blaze Mode moves each cut boundary to a nearby usable video keyframe. This keeps edits compatible with stream-copy export, which copies the encoded streams instead of re-encoding the video.");
                                            help_card(ui, "Create a region", "Move to the start and press S. Move to the end and press D. Pressing S again before D cancels the pending start. The completed region appears on the timeline.");
                                            help_card(ui, "Refine boundaries", "Drag a region's start or end handle. In Blaze Mode, the boundary continues to snap to a valid keyframe. The available keyframes limit how exact a cut can be.");
                                            help_card(ui, "Silence detection", "Detection Level controls how sensitive the scan is. Minimum Silence Duration filters out short pauses. Padding preserves audio around detected pauses. Expert Mode exposes the threshold, peak ceiling and separate before/after padding controls.");
                                            help_card(ui, "Best for", "Fast editing when preserving the original encoded video and avoiding re-encoding matter more than frame-exact cut placement.");
                                        }
                                        HelpTab::Precision => {
                                            help_page_heading(ui, "Precision Mode", "Place edit boundaries at the current playback position.");
                                            help_card(ui, "How cuts work", "Precision Mode uses the playhead's current position instead of snapping to keyframes. This allows finer placement, but a rendered export may need to re-encode portions of the video.");
                                            help_card(ui, "Create and refine a region", "Move to the desired start and press S. Move to the desired end and press D. Drag either boundary handle to refine it; boundaries are not keyframe-snapped in this mode.");
                                            help_card(ui, "Silence detection", "Use the same detection controls to find candidate pauses. Detection Level, Minimum Silence Duration and Padding determine which pauses are marked and how much surrounding audio is retained. Review the results before exporting.");
                                            help_card(ui, "Export status", "Precision video export is currently marked Coming Soon. You can still work with precise edit regions and use the available timestamp/NLE interchange exports.");
                                        }
                                        HelpTab::Timeline => {
                                            help_page_heading(ui, "Timeline & navigation", "Navigate the waveform, inspect edits and control the workspace.");
                                            help_card(ui, "Seek and scrub", "Click the waveform lane or time ruler to seek. Drag either one to scrub through the video. The ruler adjusts its tick spacing to keep time labels readable as you zoom.");
                                            help_card(ui, "Zoom and pan", "Ctrl + scroll zooms around the pointer position. Ordinary mouse-wheel scrolling pans horizontally; horizontal trackpad scrolling is supported too. During playback, the timeline follows the playhead when it moves outside the visible range.");
                                            help_card(ui, "Edit regions and playhead", "Each completed region has draggable start and end handles. Blaze Mode snaps moved handles to keyframes; Precision Mode does not. A pending S start marker can also be dragged before pressing D.");
                                            help_card(ui, "Skip Cuts", "Toggle Skip Cuts in the transport controls to automatically jump over marked regions during playback. This changes playback navigation only; it does not delete or modify the regions.");
                                            help_card(ui, "Resize the workspace", "Drag the vertical divider between the preview and Tools panel to resize them. Drag the horizontal divider above the timeline to change its height. Double-click either divider to restore its default size.");
                                        }
                                        HelpTab::KeyboardMouse => {
                                            help_page_heading(ui, "Keyboard & Mouse", "Shortcuts and pointer gestures for playback, cutting and timeline navigation.");
                                            help_section_label(ui, "KEYBOARD SHORTCUTS");
                                            help_shortcut(ui, "Space", "Play or pause the loaded video.");
                                            help_shortcut(ui, "S", "Set a cut start at the playhead. Press S again to cancel a pending start.");
                                            help_shortcut(ui, "D", "Complete the pending cut at the playhead.");
                                            help_shortcut(ui, "Ctrl + H", "Open or close Quick Help.");
                                            help_section_label(ui, "MOUSE & TRACKPAD GESTURES");
                                            help_card(ui, "Ctrl + scroll", "Zoom the timeline around the pointer position.");
                                            help_card(ui, "Scroll", "Pan horizontally through the timeline.");
                                            help_card(ui, "Click or drag", "Seek or scrub using the waveform or time ruler.");
                                            help_card(ui, "Double-click a divider", "Reset the preview/Tools split or timeline height.");
                                        }
                                        HelpTab::Export => {
                                            help_page_heading(ui, "Export & interchange", "Choose between a rendered video and an editable timeline handoff.");
                                            help_card(ui, "Blaze Export", "In Blaze Mode, export uses stream-copy operations. It preserves the original encoded video quality without re-encoding. A loaded source, usable keyframes and at least one completed edit region are required.");
                                            help_card(ui, "NLE timeline interchange", "Export an editable timeline for Adobe Premiere Pro or DaVinci Resolve using FCP7 XML, Final Cut Pro using FCPXML 1.14, OpenTimelineIO using .otio, or CMX 3600 EDL. These files contain edit decisions and source-media references; they are not rendered videos.");
                                            help_card(ui, "Current interchange model", "Blaze exports one source media file as a single video track with an optional linked audio track. The current model does not include effects, transitions, multiple source clips or multi-track edits. OpenTimelineIO is supported by current Premiere Pro and DaVinci Resolve releases.");
                                            help_card(ui, "Precision Export", "A re-encoded video export for Precision Mode is not available yet and is shown as Coming Soon.");
                                            help_section_label(ui, "BEFORE EXPORTING");
                                            help_card(ui, "Review the cut list", "Check every region on the timeline and confirm the start and end boundaries.");
                                            help_card(ui, "Choose the right output", "Use Blaze Mode for keyframe-aligned stream-copy output, or an NLE interchange file to continue editing in a supported video editor.");
                                        }
                                        HelpTab::Activation => {
                                            help_page_heading(ui, "Activation & license transfers", "Activate Blaze with your Gumroad license and keep access when moving devices.");
                                            help_card(ui, "Activate Blaze", "Purchase Blaze to receive a license key. Open the License tab in the Tools panel, paste your key and select Activate Blaze.");
                                            help_card(ui, "Move to another device", "If you reinstall Blaze, replace your computer or switch operating systems, contact us to request a license transfer. Include your purchase email and let us know which device or operating system you are moving to.");
                                             copy_support_email(ui);
                                        }
                                    }
                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new("Blaze changes as features ship. This guide describes the controls currently available in this build.")
                                            .font(FontId::new(11.5, FontFamily::Proportional))
                                            .color(ui.visuals().weak_text_color()),
                                    );
                                    ui.add_space(8.0);
                                });
                        });
                    });
                });
        }

        if self.show_license_dialog {
            let mut close_license_dialog = false;
            let mut activate_license_requested = false;
            egui::Window::new("Activate Blaze")
                .collapsible(false)
                .resizable(false)
                .default_width(390.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut self.show_license_dialog)
                .show(ui.ctx(), |ui| {
                    ui.label("Enter your license key to activate Blaze.");
                    ui.add_space(8.0);
                    ui.add(egui::TextEdit::singleline(&mut self.license_key_input)
                        .hint_text("Paste your license key")
                        .desired_width(f32::INFINITY));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(
                            !self.license_key_input.trim().is_empty()
                                && !self.license_verifying
                                && !self.license_activated,
                            egui::Button::new(if self.license_verifying { "Checking…" } else { "Activate" })
                                .min_size(egui::vec2(105.0, 32.0)),
                        ).clicked() {
                            activate_license_requested = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close_license_dialog = true;
                        }
                    });
                    if let Some(message) = &self.license_notice {
                        ui.add_space(6.0);
                        ui.colored_label(egui::Color32::from_rgb(235, 183, 113), message);
                    }
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Your key is verified online with the Blaze licensing service.")
                        .size(11.0).color(ui.visuals().weak_text_color()));
                });
            if activate_license_requested {
                self.activate_license_key(ui.ctx());
            }
            if self.license_activated {
                close_license_dialog = true;
            }
            if close_license_dialog {
                self.show_license_dialog = false;
            }
        }

    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        self.playback.shutdown(gl);
    }
}

#[derive(Clone, Copy)]
enum PlayerIcon {
    Play,
    Pause,
}

fn icon_button_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    icon: PlayerIcon,
    tooltip: &str,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    paint_player_icon(ui, rect, &response, icon);
    response.on_hover_text(tooltip)
}

fn allocated_icon_button(
    ui: &mut egui::Ui,
    id_source: &'static str,
    size: egui::Vec2,
    icon: PlayerIcon,
    tooltip: &str,
) -> egui::Response {
    let (id, rect) = ui.allocate_space(size);
    icon_button_at(ui, rect, id.with(id_source), icon, tooltip)
}

fn paint_player_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    response: &egui::Response,
    icon: PlayerIcon,
) {
    let visuals = ui.visuals();
    let widget = if response.is_pointer_button_down_on() {
        &visuals.widgets.active
    } else if response.hovered() {
        &visuals.widgets.hovered
    } else {
        &visuals.widgets.inactive
    };

    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(5), widget.bg_fill);

    let center = rect.center();
    let color = visuals.text_color();
    match icon {
        PlayerIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x - 4.5, center.y - 7.0),
                    egui::pos2(center.x + 7.0, center.y),
                    egui::pos2(center.x - 4.5, center.y + 7.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        PlayerIcon::Pause => {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(center.x - 3.5, center.y),
                    egui::vec2(3.5, 14.0),
                ),
                egui::CornerRadius::same(1),
                color,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(center.x + 3.5, center.y),
                    egui::vec2(3.5, 14.0),
                ),
                egui::CornerRadius::same(1),
                color,
            );
        }
    }
}

fn default_tools_width(root_rect: egui::Rect, info: &MediaInfo) -> f32 {
    let aspect = info
        .width
        .zip(info.height)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| width as f32 / height as f32)
        .unwrap_or(16.0 / 9.0);
    let preview_height = (root_rect.height() - 220.0 - 54.0).max(160.0);
    let preview_width = (preview_height * aspect + 14.0)
        .clamp(240.0, (root_rect.width() - 420.0).max(240.0));
    (root_rect.width() - preview_width).max(420.0)
}

fn format_ruler_time(seconds: f64, step: f64) -> String {
    let seconds = seconds.max(0.0);
    if step >= 60.0 {
        let whole_minutes = (seconds / 60.0).floor() as u64;
        let remainder = (seconds - whole_minutes as f64 * 60.0).round() as u64;
        if remainder == 0 {
            format!("{whole_minutes}m")
        } else {
            format!("{whole_minutes}m {remainder}s")
        }
    } else if step >= 1.0 {
        format!("{}s", seconds.round() as u64)
    } else {
        format!("{:.1}s", seconds)
    }
}

fn format_time_millis(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let seconds = (total_millis / 1000) % 60;
    let millis = total_millis % 1000;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes:02}:{seconds:02}.{millis:03}")
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([820.0, 580.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(|cc| {
            let app = BlazeApp::new(cc)?;
            Ok(Box::new(app))
        }),
    )
}
