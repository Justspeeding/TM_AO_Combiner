#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions, Vec2};
use eframe::{App, CreationContext, Frame, NativeOptions};
use image::{DynamicImage, GenericImageView, ImageReader, RgbaImage};
use image_dds::{dds_from_image, ImageFormat as DdsFormat, Mipmaps, Quality};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Png,
    Tga,
    DdsBc3,
    DdsBc7,
    DdsRgba8,
}

impl OutputFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Tga => "TGA",
            Self::DdsBc3 => "DDS - BC3 / DXT5",
            Self::DdsBc7 => "DDS - BC7",
            Self::DdsRgba8 => "DDS - RGBA8",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Tga => "tga",
            _ => "dds",
        }
    }
}

struct TextureSlot {
    path: Option<PathBuf>,
    image: Option<DynamicImage>,
}

impl TextureSlot {
    fn empty() -> Self {
        Self { path: None, image: None }
    }

    fn load(&mut self, path: PathBuf) -> Result<()> {
        let img = ImageReader::open(&path)
            .with_context(|| format!("Cannot open {}", path.display()))?
            .with_guessed_format()?
            .decode()
            .with_context(|| format!("Cannot decode {}", path.display()))?;
        self.path = Some(path);
        self.image = Some(img);
        Ok(())
    }

    fn clear(&mut self) {
        self.path = None;
        self.image = None;
    }
}

struct CombinerApp {
    rgb: TextureSlot,
    ao: TextureSlot,
    output_folder: Option<PathBuf>,
    output_format: OutputFormat,
    ao_strength: f32,
    invert_ao: bool,
    linear_multiply: bool,
    keep_alpha: bool,
    generate_mipmaps: bool,
    dds_quality: Quality,
    status: String,
    preview: Option<TextureHandle>,
    preview_image: Option<RgbaImage>,
    preview_dirty: bool,
    dark_preview: bool,
}

impl CombinerApp {
    fn new(_cc: &CreationContext<'_>) -> Self {
        Self {
            rgb: TextureSlot::empty(),
            ao: TextureSlot::empty(),
            output_folder: None,
            output_format: OutputFormat::DdsBc3,
            ao_strength: 1.0,
            invert_ao: false,
            linear_multiply: false,
            keep_alpha: true,
            generate_mipmaps: true,
            dds_quality: Quality::Normal,
            status: "Drop an RGB texture and an AO/Prelight texture to begin.".into(),
            preview: None,
            preview_image: None,
            preview_dirty: true,
            dark_preview: false,
        }
    }

    fn load_rgb(&mut self, path: PathBuf) {
        match self.rgb.load(path.clone()) {
            Ok(_) => self.status = format!("RGB loaded: {}", path.display()),
            Err(e) => self.status = format!("RGB error: {e:#}"),
        }
        self.preview_dirty = true;
    }

    fn load_ao(&mut self, path: PathBuf) {
        match self.ao.load(path.clone()) {
            Ok(_) => self.status = format!("AO / Prelight loaded: {}", path.display()),
            Err(e) => self.status = format!("AO error: {e:#}"),
        }
        self.preview_dirty = true;
    }

    fn receive_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        for file in dropped {
            if let Some(path) = file.path {
                let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                let supported = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tga" | "dds" | "webp" | "tif" | "tiff");
                if !supported {
                    self.status = format!("Unsupported image type: {}", path.display());
                    continue;
                }
                if self.rgb.image.is_none() {
                    self.load_rgb(path);
                } else if self.ao.image.is_none() {
                    self.load_ao(path);
                } else {
                    self.status = "Both slots are occupied. Clear a slot before dropping another image.".into();
                }
            }
        }
    }

    fn build_output(&self) -> Result<RgbaImage> {
        let rgb = self.rgb.image.as_ref().context("RGB texture is missing")?.to_rgba8();
        let ao_img = self.ao.image.as_ref().context("AO / Prelight texture is missing")?;
        let ao = ao_img.to_rgba8();
        let (w, h) = rgb.dimensions();

        let ao_resized = if ao.dimensions() == (w, h) {
            ao
        } else {
            image::imageops::resize(&ao, w, h, image::imageops::FilterType::Triangle)
        };

        let mut out = RgbaImage::new(w, h);
        let strength = self.ao_strength.clamp(0.0, 1.0);

        for y in 0..h {
            for x in 0..w {
                let c = rgb.get_pixel(x, y).0;
                let a = ao_resized.get_pixel(x, y).0;
                let mut ao_value = (0.2126 * a[0] as f32 + 0.7152 * a[1] as f32 + 0.0722 * a[2] as f32) / 255.0;
                if self.invert_ao {
                    ao_value = 1.0 - ao_value;
                }
                let factor = 1.0 - strength + strength * ao_value;

                let mut rgb_out = [0u8; 3];
                for ch in 0..3 {
                    let v = if self.linear_multiply {
                        let src = srgb_to_linear(c[ch] as f32 / 255.0);
                        let mul = srgb_to_linear((ao_value * 255.0).clamp(0.0, 255.0) / 255.0);
                        linear_to_srgb(src * (1.0 - strength + strength * mul))
                    } else {
                        (c[ch] as f32 / 255.0) * factor
                    };
                    rgb_out[ch] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
                let alpha = if self.keep_alpha { c[3] } else { 255 };
                out.put_pixel(x, y, image::Rgba([rgb_out[0], rgb_out[1], rgb_out[2], alpha]));
            }
        }
        Ok(out)
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        if !self.preview_dirty {
            return;
        }
        self.preview_dirty = false;
        let Ok(image) = self.build_output() else {
            self.preview_image = None;
            self.preview = None;
            return;
        };
        let size = [image.width() as usize, image.height() as usize];
        let color = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        self.preview = Some(ctx.load_texture("combined_preview", color, TextureOptions::LINEAR));
        self.preview_image = Some(image);
    }

    fn choose_rgb(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tga", "dds", "webp", "tif", "tiff"])
            .pick_file()
        {
            self.load_rgb(path);
        }
    }

    fn choose_ao(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tga", "dds", "webp", "tif", "tiff"])
            .pick_file()
        {
            self.load_ao(path);
        }
    }

    fn choose_output(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.output_folder = Some(path);
        }
    }

    fn default_output_name(&self) -> String {
        let stem = self.rgb.path.as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("combined_texture");
        format!("{}_AO.{}", stem, self.output_format.extension())
    }

    fn export(&mut self) {
        let folder = match &self.output_folder {
            Some(x) => x.clone(),
            None => {
                self.status = "Choose an output folder first.".into();
                return;
            }
        };
        let out = match self.build_output() {
            Ok(x) => x,
            Err(e) => {
                self.status = format!("Export error: {e:#}");
                return;
            }
        };
        let path = folder.join(self.default_output_name());
        let result = match self.output_format {
            OutputFormat::Png => out.save_with_format(&path, image::ImageFormat::Png).map_err(anyhow::Error::from),
            OutputFormat::Tga => out.save_with_format(&path, image::ImageFormat::Tga).map_err(anyhow::Error::from),
            OutputFormat::DdsBc3 | OutputFormat::DdsBc7 | OutputFormat::DdsRgba8 => self.export_dds(&out, &path),
        };
        match result {
            Ok(_) => self.status = format!("Exported: {}", path.display()),
            Err(e) => self.status = format!("Export failed: {e:#}"),
        }
    }

    fn export_dds(&self, image: &RgbaImage, path: &Path) -> Result<()> {
        let format = match self.output_format {
            OutputFormat::DdsBc3 => DdsFormat::BC3RgbaUnorm,
            OutputFormat::DdsBc7 => DdsFormat::BC7RgbaUnorm,
            OutputFormat::DdsRgba8 => DdsFormat::Rgba8Unorm,
            _ => unreachable!(),
        };
        let mipmaps = if self.generate_mipmaps { Mipmaps::GeneratedAutomatic } else { Mipmaps::Disabled };
        let dds = dds_from_image(image, format, self.dds_quality, mipmaps)
            .context("DDS encoding failed")?;
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("Cannot create {}", path.display()))?;
        dds.write(&mut file).context("Cannot write DDS")?;
        Ok(())
    }

    fn slot_ui(&mut self, ui: &mut egui::Ui, title: &str, is_rgb: bool) {
        let (loaded, loaded_name, dimensions) = {
            let slot = if is_rgb { &self.rgb } else { &self.ao };
            let name = slot.path.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("Drop image here")
                .to_owned();
            let dims = slot.image.as_ref().map(|img| img.dimensions());
            (slot.image.is_some(), name, dims)
        };

        let mut browse = false;
        let mut clear = false;

        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.heading(title);
                if loaded {
                    ui.label(egui::RichText::new("READY").color(Color32::from_rgb(90, 210, 120)));
                }
            });

            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), 76.0),
                egui::Sense::hover(),
            );
            let fill = if response.hovered() {
                Color32::from_rgb(55, 70, 90)
            } else {
                Color32::from_rgb(35, 38, 45)
            };
            ui.painter().rect_filled(rect, 8.0, fill);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &loaded_name,
                egui::FontId::proportional(15.0),
                Color32::LIGHT_GRAY,
            );

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Browse...").clicked() { browse = true; }
                if loaded && ui.button("Clear").clicked() { clear = true; }
                if let Some((w, h)) = dimensions { ui.label(format!("{} × {}", w, h)); }
            });
        });

        if browse {
            if is_rgb { self.choose_rgb(); } else { self.choose_ao(); }
        }
        if clear {
            if is_rgb { self.rgb.clear(); } else { self.ao.clear(); }
            self.preview_dirty = true;
        }
    }

}

impl App for CombinerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.receive_drops(ctx);
        self.update_preview(ctx);

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("TM Texture AO Combiner");
                ui.label("RGB + AO / Prelight → Multiply");
                ui.separator();
                ui.label("TrackMania texture workflow");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.columns(2, |columns| {
                self.slot_ui(&mut columns[0], "1. RGB / COLOR", true);
                self.slot_ui(&mut columns[1], "2. AO / PRELIGHT", false);
            });

            ui.add_space(10.0);
            ui.group(|ui| {
                ui.heading("Blend settings");
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    ui.label(egui::RichText::new("MULTIPLY").strong());
                    ui.separator();
                    ui.label("AO strength");
                    if ui.add(egui::Slider::new(&mut self.ao_strength, 0.0..=1.0).show_value(true)).changed() { self.preview_dirty = true; }
                    ui.label(format!("{:.0}%", self.ao_strength * 100.0));
                });
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.invert_ao, "Invert AO").changed() { self.preview_dirty = true; }
                    if ui.checkbox(&mut self.linear_multiply, "Linear-light multiply").changed() { self.preview_dirty = true; }
                    if ui.checkbox(&mut self.keep_alpha, "Keep RGB alpha").changed() { self.preview_dirty = true; }
                });
            });

            ui.add_space(10.0);
            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Preview");
                        if ui.checkbox(&mut self.dark_preview, "Dark background").changed() {
                            ctx.request_repaint();
                        }
                    });
                    if let Some(texture) = &self.preview {
                        let available = ui.available_size();
                        let tex_size = texture.size_vec2();
                        let scale = (available.x / tex_size.x).min(available.y / tex_size.y).min(1.0);
                        let size = tex_size * scale.max(0.1);
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        if self.dark_preview {
                            ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(16, 16, 16));
                        }
                        ui.painter().image(
                            texture.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        ui.centered_and_justified(|ui| ui.label("Load both images to see the combined texture."));
                    }
                });

                columns[1].group(|ui| {
                    ui.heading("Export");
                    ui.horizontal(|ui| {
                        ui.label("Format:");
                        egui::ComboBox::from_id_salt("format")
                            .selected_text(self.output_format.label())
                            .show_ui(ui, |ui| {
                                for format in [OutputFormat::DdsBc3, OutputFormat::DdsBc7, OutputFormat::DdsRgba8, OutputFormat::Png, OutputFormat::Tga] {
                                    if ui.selectable_value(&mut self.output_format, format, format.label()).changed() { self.preview_dirty = true; }
                                }
                            });
                    });
                    if matches!(self.output_format, OutputFormat::DdsBc3 | OutputFormat::DdsBc7 | OutputFormat::DdsRgba8) {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.generate_mipmaps, "Generate mipmaps");
                            ui.label("Quality:");
                            egui::ComboBox::from_id_salt("quality")
                                .selected_text(self.dds_quality.to_string())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.dds_quality, Quality::Fast, "Fast");
                                    ui.selectable_value(&mut self.dds_quality, Quality::Normal, "Normal");
                                    ui.selectable_value(&mut self.dds_quality, Quality::Slow, "Slow");
                                });
                        });
                    }
                    ui.horizontal(|ui| {
                        let folder = self.output_folder.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No output folder selected".into());
                        ui.label(folder);
                        if ui.button("Choose folder...").clicked() { self.choose_output(); }
                    });
                    ui.label(format!("Output name: {}", self.default_output_name()));
                    ui.add_space(6.0);
                    let ready = self.rgb.image.is_some() && self.ao.image.is_some() && self.output_folder.is_some();
                    if ui.add_enabled(ready, egui::Button::new("EXPORT TEXTURE")).clicked() { self.export(); }
                    if !ready { ui.small("Load RGB + AO and choose an output folder."); }
                });
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Status:").strong());
                ui.label(&self.status);
            });
        });

        if ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl) {
            self.export();
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn main() -> eframe::Result<()> {
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(Vec2::new(1180.0, 820.0))
            .with_min_inner_size(Vec2::new(900.0, 650.0))
            .with_title("TM Texture AO Combiner"),
        ..Default::default()
    };
    eframe::run_native(
        "TM Texture AO Combiner",
        options,
        Box::new(|cc| Ok(Box::new(CombinerApp::new(cc)))),
    )
}
