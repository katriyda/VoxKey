//! The bottom-center floating capsule. The animated window itself is managed
//! by `app`; this module owns state and painting.

use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};

pub const WINDOW_HEIGHT: f32 = 92.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Info,
    Download,
    Listening,
    Working,
    Success,
    Error,
}

pub struct Overlay {
    pub kind: OverlayKind,
    pub title: String,
    pub detail: String,
    pub alpha: f32,
    pub closing: bool,
    pub hide_at: Option<Instant>,
    level: f32,
    smooth_level: f32,
    bars: [f32; 5],
    shimmer: f32,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            kind: OverlayKind::Info,
            title: String::new(),
            detail: String::new(),
            alpha: 0.0,
            closing: false,
            hide_at: None,
            level: 0.0,
            smooth_level: 0.0,
            bars: [4.0; 5],
            shimmer: 0.0,
        }
    }
}

impl Overlay {
    pub fn present(
        &mut self,
        kind: OverlayKind,
        title: impl Into<String>,
        detail: impl Into<String>,
        hide_after: Option<Duration>,
    ) {
        self.kind = kind;
        self.title = sanitize_line(title.into());
        self.detail = sanitize_line(detail.into());
        self.closing = false;
        self.hide_at = hide_after.map(|duration| Instant::now() + duration);
        if self.alpha < 0.02 {
            self.alpha = 0.02;
        }
        if kind != OverlayKind::Listening {
            self.level = 0.0;
        }
    }

    pub fn set_transcript(&mut self, text: impl Into<String>) {
        self.title = sanitize_line(text.into());
        self.hide_at = None;
        self.closing = false;
    }

    pub fn set_detail(&mut self, detail: impl Into<String>) {
        self.detail = sanitize_line(detail.into());
    }

    pub fn set_level(&mut self, level: f32) {
        self.level = level.clamp(0.0, 1.0);
    }

    pub fn schedule_hide(&mut self, after: Duration) {
        self.hide_at = Some(Instant::now() + after);
    }

    pub fn is_fading(&self) -> bool {
        self.alpha > 0.001
    }

    pub fn is_visible(&self) -> bool {
        self.alpha > 0.001
    }

    /// Updates animations. Returns `true` when the OS window should now be
    /// hidden because the exit animation has finished.
    pub fn advance(&mut self, dt: f32) -> bool {
        self.shimmer += dt;
        let visual = (self.level * 8.0).min(1.0);
        self.smooth_level += (visual - self.smooth_level) * (dt * 12.0).min(1.0);

        for (index, bar) in self.bars.iter_mut().enumerate() {
            let phase = self.shimmer * 7.5 + index as f32 * 0.9;
            let target = match self.kind {
                OverlayKind::Listening => {
                    5.0 + self.smooth_level * (16.0 + 16.0 * phase.sin().abs())
                }
                OverlayKind::Working | OverlayKind::Download => {
                    5.0 + 5.0 * (phase.sin() * 0.5 + 0.5)
                }
                OverlayKind::Info => 5.0,
                OverlayKind::Success => 12.0 - index as f32,
                OverlayKind::Error => 5.0,
            };
            *bar += (target - *bar) * (dt * 18.0).min(1.0);
        }

        if let Some(deadline) = self.hide_at {
            if Instant::now() >= deadline {
                self.hide_at = None;
                self.closing = true;
            }
        }

        if self.closing {
            self.alpha -= dt / 0.16;
            if self.alpha <= 0.0 {
                self.alpha = 0.0;
                self.level = 0.0;
                self.closing = false;
                return true;
            }
        } else if !self.title.is_empty() || !self.detail.is_empty() {
            self.alpha += dt / 0.18;
            if self.alpha > 1.0 {
                self.alpha = 1.0;
            }
        }
        false
    }

    pub fn desired_width(&self, ctx: &egui::Context) -> f32 {
        let main = if self.title.is_empty() {
            "VoxKey"
        } else {
            &self.title
        };
        let main_width = ctx.fonts(|fonts| {
            fonts
                .layout_no_wrap(main.to_string(), FontId::proportional(14.5), Color32::WHITE)
                .size()
                .x
        });
        let detail_width = if self.detail.is_empty() {
            0.0
        } else {
            ctx.fonts(|fonts| {
                fonts
                    .layout_no_wrap(
                        self.detail.clone(),
                        FontId::proportional(10.5),
                        Color32::WHITE,
                    )
                    .size()
                    .x
            })
        };
        (f32::max(main_width, detail_width) + 152.0).clamp(304.0, 640.0)
    }

    pub fn render(&self, ctx: &egui::Context) {
        if self.alpha <= 0.001 {
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let screen = ui.max_rect();
                let animate = self.alpha.clamp(0.0, 1.0);
                let slide = (1.0 - animate) * 10.0;
                let capsule = Rect::from_min_max(
                    Pos2::new(screen.left() + 10.0, screen.top() + 8.0 + slide),
                    Pos2::new(screen.right() - 10.0, screen.bottom() - 8.0 + slide),
                );
                let painter = ui.painter().with_clip_rect(screen);
                let radius = CornerRadius::same(38);

                // Soft shadow and glass body.
                painter.rect_filled(
                    capsule.translate(Vec2::new(0.0, 3.0)),
                    radius,
                    fade(Color32::from_black_alpha(115), animate),
                );
                painter.rect_filled(
                    capsule,
                    radius,
                    fade(Color32::from_rgba_unmultiplied(20, 23, 31, 232), animate),
                );
                painter.rect_stroke(
                    capsule,
                    radius,
                    Stroke::new(
                        1.0_f32,
                        fade(Color32::from_rgba_unmultiplied(255, 255, 255, 30), animate),
                    ),
                    StrokeKind::Inside,
                );
                // A top inner highlight gives the capsule a glossy edge.
                let highlight = Rect::from_min_max(
                    Pos2::new(capsule.left() + 20.0, capsule.top() + 1.0),
                    Pos2::new(capsule.right() - 20.0, capsule.top() + 2.0),
                );
                painter.rect_filled(
                    highlight,
                    CornerRadius::same(1),
                    fade(Color32::from_rgba_unmultiplied(255, 255, 255, 22), animate),
                );

                let accent = self.accent();
                self.paint_waveform(&painter, capsule, accent, animate);

                let text_left = capsule.left() + 112.0;
                let divider_x = text_left - 18.0;
                painter.line_segment(
                    [
                        Pos2::new(divider_x, capsule.top() + 22.0),
                        Pos2::new(divider_x, capsule.bottom() - 22.0),
                    ],
                    Stroke::new(
                        1.0_f32,
                        fade(Color32::from_rgba_unmultiplied(255, 255, 255, 20), animate),
                    ),
                );

                let text_rect = Rect::from_min_max(
                    Pos2::new(text_left, capsule.top() + 12.0),
                    Pos2::new(capsule.right() - 20.0, capsule.bottom() - 10.0),
                );
                let clipped_painter = painter.with_clip_rect(text_rect);

                let has_detail = !self.detail.is_empty();
                let main_color = match self.kind {
                    OverlayKind::Error => Color32::from_rgb(255, 186, 177),
                    OverlayKind::Success => Color32::from_rgb(213, 255, 226),
                    _ => Color32::from_rgb(240, 243, 248),
                };
                let main_font = FontId::proportional(14.5);
                let main_render = fade(main_color, animate);
                let main_galley = ctx.fonts(|fonts| {
                    fonts.layout_no_wrap(self.title.clone(), main_font.clone(), main_render)
                });
                let main_pos = if has_detail {
                    Pos2::new(text_left, capsule.center().y - 19.0)
                } else {
                    Pos2::new(
                        text_left,
                        capsule.center().y - main_galley.size().y * 0.5 + 1.0,
                    )
                };
                let main_x = if main_galley.size().x > text_rect.width() {
                    text_rect.right() - main_galley.size().x
                } else {
                    text_left
                };
                clipped_painter.galley(Pos2::new(main_x, main_pos.y), main_galley, main_render);

                if has_detail {
                    let detail_color = Color32::from_rgb(151, 160, 177);
                    let detail_render = fade(detail_color, animate);
                    let detail_galley = ctx.fonts(|fonts| {
                        fonts.layout_no_wrap(
                            self.detail.clone(),
                            FontId::proportional(10.5),
                            detail_render,
                        )
                    });
                    let x = if detail_galley.size().x > text_rect.width() {
                        text_rect.right() - detail_galley.size().x
                    } else {
                        text_left
                    };
                    clipped_painter.galley(
                        Pos2::new(x, capsule.center().y + 3.0),
                        detail_galley,
                        detail_render,
                    );
                }

                // Tiny state dot after the detail or at the right edge.
                let dot_center = Pos2::new(capsule.right() - 22.0, capsule.center().y);
                painter.circle_filled(dot_center, 3.0, fade(accent, animate * 0.9));
            });
    }

    fn paint_waveform(
        &self,
        painter: &egui::Painter,
        capsule: Rect,
        accent: Color32,
        animate: f32,
    ) {
        let center = Pos2::new(capsule.left() + 58.0, capsule.center().y);
        for (index, height) in self.bars.iter().enumerate() {
            let x = center.x + index as f32 * 9.0 - 22.0;
            let h = height * animate.clamp(0.35, 1.0);
            let bar = Rect::from_center_size(Pos2::new(x, center.y), Vec2::new(5.0, h.max(4.0)));
            let color = if index == 2 {
                accent
            } else {
                Color32::from_rgba_unmultiplied(
                    accent.r(),
                    accent.g(),
                    accent.b(),
                    150 + index as u8 * 15,
                )
            };
            painter.rect_filled(bar, CornerRadius::same(3), fade(color, animate));
        }
    }

    fn accent(&self) -> Color32 {
        match self.kind {
            OverlayKind::Listening => Color32::from_rgb(112, 231, 173),
            OverlayKind::Download | OverlayKind::Info => Color32::from_rgb(120, 181, 255),
            OverlayKind::Working => Color32::from_rgb(255, 202, 112),
            OverlayKind::Success => Color32::from_rgb(127, 235, 166),
            OverlayKind::Error => Color32::from_rgb(255, 125, 125),
        }
    }
}

fn fade(color: Color32, opacity: f32) -> Color32 {
    let alpha = (color.a() as f32 * opacity.clamp(0.0, 1.0)) as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn sanitize_line(text: String) -> String {
    text.chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string()
}
