//! Smooth macOS script text rendering.
//!
//! GPUI's default macOS text shader blends raw CoreGraphics masks in sRGB without
//! gamma or contrast correction, and CoreGraphics applies asymmetric stem
//! dilation whenever `AppleFontSmoothing` is active. Here we disable smoothing
//! dilation, shape and wrap paragraphs via CoreText, and composite each glyph
//! as a device-pixel-aligned `RenderImage` whose alpha curve applies
//! DirectWrite / `gpui_wgpu`-style `gamma = 1.8` perceptual correction.

/// Compute the 256-entry perceptual alpha LUT matching `gpui_wgpu` / DirectWrite
/// `gamma = 1.8` contrast and gamma correction (`get_gamma_correction_ratios` +
/// `apply_contrast_and_gamma_correction`) for a foreground sRGB colour.
pub fn build_alpha_lut(fg_r: f32, fg_g: f32, fg_b: f32, enhanced_contrast: f32) -> [u8; 256] {
    // Exact ratios from gpui::get_gamma_correction_ratios(1.8).
    const RATIOS: [f32; 4] = [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0];
    const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
    const NORM24: f32 = ((0x100 as f64) / 255.0 * 4.0) as f32;
    let g = [
        RATIOS[0] * NORM13,
        RATIOS[1] * NORM24,
        RATIOS[2] * NORM13,
        RATIOS[3] * NORM24,
    ];

    let brightness = 0.30 * fg_r + 0.59 * fg_g + 0.11 * fg_b;
    let multiplier = (4.0 * (0.75 - brightness)).clamp(0.0, 1.0);
    let k = enhanced_contrast * multiplier;

    let mut lut = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let a = i as f32 / 255.0;
        let contrasted = a * (k + 1.0) / (a * k + 1.0);
        let brightness_adj = g[0] * brightness + g[1];
        let correction = brightness_adj * contrasted + (g[2] * brightness + g[3]);
        let corrected = (contrasted + contrasted * (1.0 - contrasted) * correction).clamp(0.0, 1.0);
        lut[i] = (corrected * 255.0).round() as u8;
        i += 1;
    }
    lut[0] = 0;
    lut[255] = 255;
    lut
}

/// Quantize a horizontal device-pixel coordinate into an integer pixel origin
/// and a quarter-pixel phase `0..4`.
pub fn quantize_subpixel_x(x_dp: f32) -> (i32, u8) {
    let steps = (x_dp * 4.0).round() as i64;
    let origin = steps.div_euclid(4) as i32;
    let phase = steps.rem_euclid(4) as u8;
    (origin, phase)
}

/// Map a CSS numeric font weight (`100..=900`) to CoreText's `kCTFontWeightTrait`
/// (`-1.0..=1.0`) using Apple's AppKit `NSFontWeight` anchors.
pub fn weight_to_ct_trait(weight: f32) -> f64 {
    const STOPS: &[(f32, f64)] = &[
        (100.0, -0.80),
        (200.0, -0.60),
        (300.0, -0.40),
        (400.0, 0.00),
        (500.0, 0.23),
        (600.0, 0.30),
        (700.0, 0.40),
        (800.0, 0.56),
        (900.0, 0.62),
    ];
    let w = weight.clamp(100.0, 900.0);
    for pair in STOPS.windows(2) {
        let (w0, t0) = pair[0];
        let (w1, t1) = pair[1];
        if w <= w1 {
            let frac = ((w - w0) / (w1 - w0)) as f64;
            return t0 + (t1 - t0) * frac;
        }
    }
    0.0
}

/// Convert sorted UTF-8 byte spans into UTF-16 `(start, len, bold, italic)`
/// ranges for `CFAttributedString`.
pub fn utf8_spans_to_utf16(
    text: &str,
    spans: &[(usize, usize, bool, bool)],
) -> Vec<(usize, usize, bool, bool)> {
    if spans.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let mut boundaries: Vec<(usize, usize)> = Vec::with_capacity(spans.len() * 2);
    for &(start, end, _, _) in spans {
        boundaries.push((start.min(text.len()), 0));
        boundaries.push((end.min(text.len()), 0));
    }
    boundaries.sort_unstable_by_key(|&(byte, _)| byte);
    boundaries.dedup_by_key(|pair| pair.0);

    let mut utf16_pos = 0usize;
    let mut b_idx = 0usize;
    for (byte_pos, ch) in text.char_indices() {
        while b_idx < boundaries.len() && boundaries[b_idx].0 <= byte_pos {
            boundaries[b_idx].1 = utf16_pos;
            b_idx += 1;
        }
        if b_idx >= boundaries.len() {
            break;
        }
        utf16_pos += ch.len_utf16();
    }
    while b_idx < boundaries.len() {
        boundaries[b_idx].1 = utf16_pos;
        b_idx += 1;
    }

    let lookup = |byte: usize| -> usize {
        let clamped = byte.min(text.len());
        match boundaries.binary_search_by_key(&clamped, |&(b, _)| b) {
            Ok(i) => boundaries[i].1,
            Err(_) => utf16_pos,
        }
    };

    spans
        .iter()
        .filter_map(|&(start, end, bold, italic)| {
            let u_start = lookup(start);
            let u_end = lookup(end);
            (u_end > u_start).then_some((u_start, u_end - u_start, bold, italic))
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{build_alpha_lut, quantize_subpixel_x, utf8_spans_to_utf16, weight_to_ct_trait};
    use crate::markdown::StyledSpan;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::attributed_string::{CFAttributedStringRef, CFMutableAttributedString};
    use core_foundation::base::{CFIndex, CFRange, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::base::{CGFloat, kCGImageAlphaPremultipliedLast};
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::{CGContext, CGTextDrawingMode};
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::{CGPoint, CGRect};
    use core_text::font::{self, CTFont, CTFontRef};
    use core_text::font_descriptor::{
        CTFontDescriptor, CTFontDescriptorRef, SymbolicTraitAccessors, TraitAccessors,
        kCTFontColorGlyphsTrait, kCTFontFamilyNameAttribute, kCTFontItalicTrait,
        kCTFontOrientationDefault, kCTFontSlantTrait, kCTFontVariationAttribute,
        kCTFontWeightTrait,
    };
    use core_text::line::{CTLine, CTLineRef};
    use core_text::string_attributes::kCTFontAttributeName;
    use gpui::{
        App, Bounds, Corners, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
        LayoutId, Pixels, Point, RenderImage, Rgba, SharedString, Size, Window, point, px, size,
    };
    use image::{Frame, RgbaImage};
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use smallvec::SmallVec;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::ffi::c_void;
    use std::hash::{Hash, Hasher};
    use std::ptr::null;
    use std::rc::Rc;
    use std::sync::Arc;

    const MAX_CACHED_GLYPHS: usize = 2048;
    const MAX_CACHED_PARAGRAPHS: usize = 512;

    const CG_IMAGE_ALPHA_ONLY: u32 = 7;

    #[repr(C)]
    struct __CTTypesetter(c_void);
    type CTTypesetterRef = *const __CTTypesetter;

    struct Typesetter(CTTypesetterRef);

    impl Drop for Typesetter {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
            }
        }
    }

    extern "C" {
        fn CTTypesetterCreateWithAttributedString(string: CFAttributedStringRef)
            -> CTTypesetterRef;
        fn CTTypesetterSuggestLineBreak(
            typesetter: CTTypesetterRef,
            start_index: CFIndex,
            width: f64,
        ) -> CFIndex;
        fn CTTypesetterCreateLine(typesetter: CTTypesetterRef, string_range: CFRange) -> CTLineRef;
        fn CTFontDescriptorCreateMatchingFontDescriptors(
            descriptor: CTFontDescriptorRef,
            mandatory_attributes: *const c_void,
        ) -> CFArrayRef;
        fn CTFontDescriptorCopyAttribute(
            descriptor: CTFontDescriptorRef,
            attribute: CFStringRef,
        ) -> CFTypeRef;
        fn CTFontCopyVariation(font: CTFontRef) -> CFDictionaryRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[derive(Clone, Eq, PartialEq, Hash)]
    struct FontStyleKey {
        family: Option<String>,
        size_centipts: u32,
        weight_tenths: i32,
        italic: bool,
    }

    #[derive(Clone, Eq, PartialEq, Hash)]
    struct GlyphKey {
        font_ps: String,
        wght_centi: i32,
        size_centipts: u32,
        scale_centi: u32,
        glyph: CGGlyph,
        subpixel_x: u8,
        is_color: bool,
    }

    #[derive(Clone)]
    struct CachedGlyph {
        image: Option<Arc<RenderImage>>,
        offset_dp: Point<i32>,
        size_dp: Size<i32>,
    }

    struct GlyphRunData {
        font: CTFont,
        font_ps: String,
        wght_centi: i32,
        is_color: bool,
        glyphs: Vec<CGGlyph>,
        positions: Vec<CGPoint>,
    }

    struct WrappedLineData {
        ascent: f32,
        descent: f32,
        width: f32,
        runs: Vec<GlyphRunData>,
    }

    #[derive(Clone, Eq, PartialEq, Hash)]
    struct LayoutKey {
        text_hash: u64,
        text_len: usize,
        spans_hash: u64,
        family: Option<String>,
        size_centipts: u32,
        base_weight: i32,
        bold_weight: i32,
        wrap_width_centipts: i32,
    }

    struct CachedLayout {
        lines: Arc<Vec<WrappedLineData>>,
        max_line_width: f32,
    }

    #[derive(Clone, Eq, PartialEq)]
    struct ActiveConfig {
        family: Option<String>,
        size_centipts: u32,
        base_weight: i32,
        scale_centi: u32,
        fg_bgra: [u8; 3],
    }

    /// Per-window CoreText layout and gamma-corrected glyph tile cache.
    pub struct SmoothTextRenderer {
        fonts: HashMap<FontStyleKey, CTFont>,
        glyphs: HashMap<GlyphKey, CachedGlyph>,
        glyph_order: VecDeque<GlyphKey>,
        layouts: HashMap<LayoutKey, CachedLayout>,
        layout_order: VecDeque<LayoutKey>,
        pending_drops: Vec<Arc<RenderImage>>,
        alpha_lut: [u8; 256],
        fg_bgra: [u8; 3],
        active: Option<ActiveConfig>,
    }

    impl SmoothTextRenderer {
        pub fn new(color: Hsla) -> Self {
            let (fg_bgra, alpha_lut) = color_palette(color);
            Self {
                fonts: HashMap::new(),
                glyphs: HashMap::new(),
                glyph_order: VecDeque::new(),
                layouts: HashMap::new(),
                layout_order: VecDeque::new(),
                pending_drops: Vec::new(),
                alpha_lut,
                fg_bgra,
                active: None,
            }
        }

        fn sync_config(
            &mut self,
            family: Option<&str>,
            font_size: f32,
            base_weight: f32,
            color: Hsla,
            scale_factor: f32,
        ) {
            let (fg_bgra, alpha_lut) = color_palette(color);
            let next = ActiveConfig {
                family: family.map(str::to_owned),
                size_centipts: (font_size * 100.0).round() as u32,
                base_weight: (base_weight * 10.0).round() as i32,
                scale_centi: (scale_factor * 100.0).round() as u32,
                fg_bgra,
            };
            if self.active.as_ref() == Some(&next) {
                return;
            }
            for (_, entry) in self.glyphs.drain() {
                if let Some(img) = entry.image {
                    self.pending_drops.push(img);
                }
            }
            self.glyph_order.clear();
            self.layouts.clear();
            self.layout_order.clear();
            self.fonts.clear();
            self.fg_bgra = fg_bgra;
            self.alpha_lut = alpha_lut;
            self.active = Some(next);
        }

        pub fn flush_drops(&mut self, window: &mut Window) {
            for img in self.pending_drops.drain(..) {
                let _ = window.drop_image(img);
            }
        }

        fn resolve_font(
            &mut self,
            family: Option<&str>,
            font_size: f32,
            weight: f32,
            italic: bool,
        ) -> CTFont {
            let key = FontStyleKey {
                family: family.map(str::to_owned),
                size_centipts: (font_size * 100.0).round() as u32,
                weight_tenths: (weight * 10.0).round() as i32,
                italic,
            };
            if let Some(font) = self.fonts.get(&key) {
                return font.clone();
            }
            let ct_font = create_ct_font(family, font_size, weight, italic);
            self.fonts.insert(key, ct_font.clone());
            ct_font
        }

        fn layout_paragraph(
            &mut self,
            text: &str,
            spans: &[StyledSpan],
            family: Option<&str>,
            font_size: f32,
            base_weight: f32,
            bold_weight: f32,
            wrap_width: f32,
        ) -> (Arc<Vec<WrappedLineData>>, f32) {
            let wrap_centi = (wrap_width.max(1.0) * 100.0).round() as i32;
            let key = LayoutKey {
                text_hash: hash_str(text),
                text_len: text.len(),
                spans_hash: hash_spans(spans),
                family: family.map(str::to_owned),
                size_centipts: (font_size * 100.0).round() as u32,
                base_weight: (base_weight * 10.0).round() as i32,
                bold_weight: (bold_weight * 10.0).round() as i32,
                wrap_width_centipts: wrap_centi,
            };
            if let Some(cached) = self.layouts.get(&key) {
                return (cached.lines.clone(), cached.max_line_width);
            }

            let regular = self.resolve_font(family, font_size, base_weight, false);
            let lines = if text.is_empty() {
                vec![WrappedLineData {
                    ascent: regular.ascent() as f32,
                    descent: regular.descent() as f32,
                    width: 0.0,
                    runs: Vec::new(),
                }]
            } else {
                let mut attr = CFMutableAttributedString::new();
                let cf_text = CFString::new(text);
                attr.replace_str(&cf_text, CFRange::init(0, 0));
                let utf16_len = attr.char_len();
                unsafe {
                    attr.set_attribute(
                        CFRange::init(0, utf16_len),
                        kCTFontAttributeName,
                        &regular,
                    );
                }

                let raw_spans: Vec<(usize, usize, bool, bool)> = spans
                    .iter()
                    .map(|s| (s.range.start, s.range.end, s.style.bold, s.style.italic))
                    .collect();
                for (u_start, u_len, is_bold, is_italic) in utf8_spans_to_utf16(text, &raw_spans) {
                    let span_weight = if is_bold { bold_weight } else { base_weight };
                    let span_font = self.resolve_font(family, font_size, span_weight, is_italic);
                    unsafe {
                        attr.set_attribute(
                            CFRange::init(u_start as CFIndex, u_len as CFIndex),
                            kCTFontAttributeName,
                            &span_font,
                        );
                    }
                }

                let ts = Typesetter(unsafe {
                    CTTypesetterCreateWithAttributedString(attr.as_concrete_TypeRef())
                });
                let mut wrapped = Vec::new();
                let mut start: CFIndex = 0;
                while start < utf16_len {
                    let mut count = unsafe {
                        CTTypesetterSuggestLineBreak(ts.0, start, wrap_width.max(1.0) as f64)
                    };
                    if count <= 0 {
                        count = 1;
                    }
                    if start + count > utf16_len {
                        count = utf16_len - start;
                    }
                    let ct_line = unsafe {
                        CTLine::wrap_under_create_rule(CTTypesetterCreateLine(
                            ts.0,
                            CFRange::init(start, count),
                        ))
                    };
                    wrapped.push(extract_line_data(&ct_line, &regular));
                    start += count;
                }
                if wrapped.is_empty() {
                    wrapped.push(WrappedLineData {
                        ascent: regular.ascent() as f32,
                        descent: regular.descent() as f32,
                        width: 0.0,
                        runs: Vec::new(),
                    });
                }
                wrapped
            };

            let max_line_width = lines
                .iter()
                .map(|l| l.width)
                .fold(0.0f32, f32::max)
                .min(wrap_width.max(1.0));
            let lines = Arc::new(lines);

            if self.layouts.len() >= MAX_CACHED_PARAGRAPHS {
                if let Some(old_key) = self.layout_order.pop_front() {
                    self.layouts.remove(&old_key);
                }
            }
            self.layouts.insert(
                key.clone(),
                CachedLayout {
                    lines: lines.clone(),
                    max_line_width,
                },
            );
            self.layout_order.push_back(key);
            (lines, max_line_width)
        }

        fn rasterize_or_get_glyph(
            &mut self,
            run: &GlyphRunData,
            glyph: CGGlyph,
            subpixel_x: u8,
            scale_factor: f32,
        ) -> Option<CachedGlyph> {
            let size_centipts = (run.font.pt_size() * 100.0).round() as u32;
            let scale_centi = (scale_factor * 100.0).round() as u32;
            let key = GlyphKey {
                font_ps: run.font_ps.clone(),
                wght_centi: run.wght_centi,
                size_centipts,
                scale_centi,
                glyph,
                subpixel_x,
                is_color: run.is_color,
            };
            if let Some(cached) = self.glyphs.get(&key) {
                return Some(cached.clone());
            }

            let entry = rasterize_glyph(
                &run.font,
                glyph,
                subpixel_x,
                scale_factor,
                run.is_color,
                self.fg_bgra,
                &self.alpha_lut,
            );

            if self.glyphs.len() >= MAX_CACHED_GLYPHS {
                if let Some(evicted) = self.glyph_order.pop_front() {
                    if let Some(old) = self.glyphs.remove(&evicted) {
                        if let Some(img) = old.image {
                            self.pending_drops.push(img);
                        }
                    }
                }
            }
            self.glyphs.insert(key.clone(), entry.clone());
            self.glyph_order.push_back(key);
            Some(entry)
        }
    }

    fn color_palette(color: Hsla) -> ([u8; 3], [u8; 256]) {
        let rgba: Rgba = color.into();
        let fg_bgra = [
            (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        let lut = build_alpha_lut(rgba.r, rgba.g, rgba.b, 1.0);
        (fg_bgra, lut)
    }

    fn create_ct_font(family: Option<&str>, font_size: f32, weight: f32, italic: bool) -> CTFont {
        let target_weight = weight_to_ct_trait(weight);
        if let Some(family_name) = family {
            let cf_family = CFString::new(family_name);
            let attrs = unsafe {
                CFDictionary::from_CFType_pairs(&[(
                    CFString::wrap_under_get_rule(kCTFontFamilyNameAttribute),
                    cf_family.as_CFType(),
                )])
            };
            let query = font::new_from_descriptor(
                &core_text::font_descriptor::new_from_attributes(&attrs),
                font_size as CGFloat,
            );
            let base_desc = query.copy_descriptor();
            let array_ref = unsafe {
                CTFontDescriptorCreateMatchingFontDescriptors(base_desc.as_concrete_TypeRef(), null())
            };
            if !array_ref.is_null() {
                let candidates: CFArray<CTFontDescriptor> =
                    unsafe { CFArray::wrap_under_create_rule(array_ref) };
                let mut best: Option<((bool, i32, bool), CTFontDescriptor, bool)> = None;
                for desc in candidates.iter() {
                    let traits = desc.traits();
                    let sym = traits.symbolic_traits();
                    let slant_key = unsafe { CFString::wrap_under_get_rule(kCTFontSlantTrait) };
                    let weight_key = unsafe { CFString::wrap_under_get_rule(kCTFontWeightTrait) };
                    let slant = traits
                        .find(&slant_key)
                        .and_then(|v| v.downcast::<CFNumber>())
                        .and_then(|n| n.to_f64())
                        .unwrap_or(0.0);
                    let w_trait = traits
                        .find(&weight_key)
                        .and_then(|v| v.downcast::<CFNumber>())
                        .and_then(|n| n.to_f64())
                        .unwrap_or(0.0);
                    let is_italic = sym.is_italic() || slant.abs() > 0.01;
                    let italic_mismatch = is_italic != italic;
                    let weight_dist = ((w_trait - target_weight).abs() * 10000.0).round() as i32;
                    let lacks_var = unsafe {
                        let attr =
                            CTFontDescriptorCopyAttribute(desc.as_concrete_TypeRef(), kCTFontVariationAttribute);
                        if attr.is_null() {
                            true
                        } else {
                            CFRelease(attr);
                            false
                        }
                    };
                    let score = (italic_mismatch, weight_dist, lacks_var);
                    let replace = best
                        .as_ref()
                        .map_or(true, |(best_score, _, _)| score < *best_score);
                    if replace {
                        best = Some((score, desc.clone(), is_italic));
                    }
                }
                if let Some((_, best_desc, matched_italic)) = best {
                    let matched = font::new_from_descriptor(&best_desc, font_size as CGFloat);
                    if italic && !matched_italic {
                        if let Some(slanted) =
                            matched.clone_with_symbolic_traits(kCTFontItalicTrait, kCTFontItalicTrait)
                        {
                            return slanted;
                        }
                    }
                    return matched;
                }
            }
            let _ = query;
        }

        // System font via AppKit so optical size + SF weights match macOS UI.
        let mut system_ct: Option<CTFont> = None;
        objc2::rc::autoreleasepool(|_| unsafe {
            let ns_font: *mut AnyObject = msg_send![
                objc2::class!(NSFont),
                systemFontOfSize: font_size as CGFloat,
                weight: target_weight as CGFloat
            ];
            if !ns_font.is_null() {
                system_ct = Some(CTFont::wrap_under_get_rule(ns_font as CTFontRef));
            }
        });
        let base = system_ct.unwrap_or_else(|| {
            font::new_ui_font_for_language(font::kCTFontSystemFontType, font_size as CGFloat, None)
        });
        if italic {
            if let Some(ital) =
                base.clone_with_symbolic_traits(kCTFontItalicTrait, kCTFontItalicTrait)
            {
                return ital;
            }
        }
        base
    }

    fn font_wght_centi(ct_font: &CTFont) -> i32 {
        let var_ref = unsafe { CTFontCopyVariation(ct_font.as_concrete_TypeRef()) };
        if var_ref.is_null() {
            return 0;
        }
        let dict: CFDictionary<CFNumber, CFNumber> =
            unsafe { CFDictionary::wrap_under_create_rule(var_ref) };
        let wght_tag = CFNumber::from(0x77676874i32); // 'wght'
        dict.find(&wght_tag)
            .and_then(|n| n.to_f64())
            .map(|v| (v * 100.0).round() as i32)
            .unwrap_or(0)
    }

    fn extract_line_data(ct_line: &CTLine, fallback_font: &CTFont) -> WrappedLineData {
        let bounds = ct_line.get_typographic_bounds();
        let attr_key = unsafe { CFString::wrap_under_get_rule(kCTFontAttributeName) };
        let mut runs = Vec::new();
        for run in ct_line.glyph_runs().iter() {
            let count = run.glyph_count();
            if count <= 0 {
                continue;
            }
            let run_font = run
                .attributes()
                .and_then(|attrs| {
                    attrs.find(&attr_key).map(|v| unsafe {
                        CTFont::wrap_under_get_rule(v.as_CFTypeRef() as CTFontRef)
                    })
                })
                .unwrap_or_else(|| fallback_font.clone());
            let is_color = (run_font.symbolic_traits() & kCTFontColorGlyphsTrait) != 0;
            runs.push(GlyphRunData {
                font_ps: run_font.postscript_name(),
                wght_centi: font_wght_centi(&run_font),
                font: run_font,
                is_color,
                glyphs: run.glyphs().into_owned(),
                positions: run.positions().into_owned(),
            });
        }
        WrappedLineData {
            ascent: (bounds.ascent as f32).max(fallback_font.ascent() as f32),
            descent: (bounds.descent as f32).max(fallback_font.descent() as f32),
            width: bounds.width as f32,
            runs,
        }
    }

    fn rasterize_glyph(
        font: &CTFont,
        glyph: CGGlyph,
        subpixel_x: u8,
        scale_factor: f32,
        is_color: bool,
        fg_bgra: [u8; 3],
        alpha_lut: &[u8; 256],
    ) -> CachedGlyph {
        let bbox: CGRect = font.get_bounding_rects_for_glyphs(kCTFontOrientationDefault, &[glyph]);
        if bbox.size.width <= 0.0 || bbox.size.height <= 0.0 {
            return CachedGlyph {
                image: None,
                offset_dp: point(0, 0),
                size_dp: size(0, 0),
            };
        }

        let s = scale_factor as f64;
        let left_dp = (bbox.origin.x * s).floor() as i32 - 1;
        let right_dp = ((bbox.origin.x + bbox.size.width) * s).ceil() as i32
            + 1
            + if subpixel_x > 0 { 1 } else { 0 };
        let top_dp = (-(bbox.origin.y + bbox.size.height) * s).floor() as i32 - 1;
        let bottom_dp = (-(bbox.origin.y) * s).ceil() as i32 + 1;

        let width_px = (right_dp - left_dp).max(0) as usize;
        let height_px = (bottom_dp - top_dp).max(0) as usize;
        if width_px == 0 || height_px == 0 {
            return CachedGlyph {
                image: None,
                offset_dp: point(0, 0),
                size_dp: size(0, 0),
            };
        }

        let subpixel_shift_pt = (subpixel_x as f64 / 4.0) / s;
        let bgra_bytes = if is_color {
            let mut rgba = vec![0u8; width_px * height_px * 4];
            let ctx = CGContext::create_bitmap_context(
                Some(rgba.as_mut_ptr() as *mut c_void),
                width_px,
                height_px,
                8,
                width_px * 4,
                &CGColorSpace::create_device_rgb(),
                kCGImageAlphaPremultipliedLast,
            );
            ctx.translate(-left_dp as CGFloat, bottom_dp as CGFloat);
            ctx.scale(s, s);
            font.draw_glyphs(&[glyph], &[CGPoint::new(subpixel_shift_pt, 0.0)], ctx);
            for px_bytes in rgba.chunks_exact_mut(4) {
                let a = px_bytes[3] as f32 / 255.0;
                let r = if a > 0.0 { (px_bytes[0] as f32 / a).min(255.0) as u8 } else { 0 };
                let g = if a > 0.0 { (px_bytes[1] as f32 / a).min(255.0) as u8 } else { 0 };
                let b = if a > 0.0 { (px_bytes[2] as f32 / a).min(255.0) as u8 } else { 0 };
                px_bytes[0] = b;
                px_bytes[1] = g;
                px_bytes[2] = r;
            }
            rgba
        } else {
            let mut alpha = vec![0u8; width_px * height_px];
            let ctx = CGContext::create_bitmap_context(
                Some(alpha.as_mut_ptr() as *mut c_void),
                width_px,
                height_px,
                8,
                width_px,
                &CGColorSpace::create_device_gray(),
                CG_IMAGE_ALPHA_ONLY,
            );
            ctx.translate(-left_dp as CGFloat, bottom_dp as CGFloat);
            ctx.scale(s, s);
            ctx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
            ctx.set_allows_antialiasing(true);
            ctx.set_should_antialias(true);
            ctx.set_allows_font_smoothing(false);
            ctx.set_should_smooth_fonts(false);
            ctx.set_allows_font_subpixel_positioning(true);
            ctx.set_should_subpixel_position_fonts(true);
            ctx.set_allows_font_subpixel_quantization(false);
            ctx.set_should_subpixel_quantize_fonts(false);
            ctx.set_gray_fill_color(0.0, 1.0);
            font.draw_glyphs(&[glyph], &[CGPoint::new(subpixel_shift_pt, 0.0)], ctx);

            // Fill every pixel with fg RGB (straight alpha) so bilinear edge
            // filtering can never introduce dark RGB fringes against transparent texels.
            let mut bgra = Vec::with_capacity(width_px * height_px * 4);
            for &cov in &alpha {
                bgra.push(fg_bgra[0]);
                bgra.push(fg_bgra[1]);
                bgra.push(fg_bgra[2]);
                bgra.push(alpha_lut[cov as usize]);
            }
            bgra
        };

        let buf = RgbaImage::from_raw(width_px as u32, height_px as u32, bgra_bytes)
            .expect("glyph buffer size matches width * height * 4");
        let render_img = Arc::new(RenderImage::new(SmallVec::from_elem(Frame::new(buf), 1)));
        CachedGlyph {
            image: Some(render_img),
            offset_dp: point(left_dp, top_dp),
            size_dp: size(width_px as i32, height_px as i32),
        }
    }

    fn hash_str(s: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        h.finish()
    }

    fn hash_spans(spans: &[StyledSpan]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        spans.len().hash(&mut h);
        for span in spans {
            span.range.start.hash(&mut h);
            span.range.end.hash(&mut h);
            span.style.bold.hash(&mut h);
            span.style.italic.hash(&mut h);
        }
        h.finish()
    }

    /// A paragraph element rendered with CoreText shaping and perceptual
    /// linear/gamma-corrected glyph tiles.
    pub struct SmoothParagraph {
        id: ElementId,
        text: SharedString,
        spans: Vec<StyledSpan>,
        family: Option<SharedString>,
        font_size: f32,
        base_weight: f32,
        bold_weight: f32,
        line_height_multiplier: f32,
        color: Hsla,
        renderer: Rc<RefCell<SmoothTextRenderer>>,
    }

    impl SmoothParagraph {
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            id: impl Into<ElementId>,
            text: SharedString,
            spans: Vec<StyledSpan>,
            family: Option<SharedString>,
            font_size: f32,
            base_weight: f32,
            bold_weight: f32,
            line_height_multiplier: f32,
            color: Hsla,
            renderer: Rc<RefCell<SmoothTextRenderer>>,
        ) -> Self {
            Self {
                id: id.into(),
                text,
                spans,
                family,
                font_size,
                base_weight,
                bold_weight,
                line_height_multiplier,
                color,
                renderer,
            }
        }
    }

    pub struct ParagraphLayoutState {
        lines: Rc<RefCell<Option<Arc<Vec<WrappedLineData>>>>>,
        line_height: Pixels,
    }

    impl Element for SmoothParagraph {
        type RequestLayoutState = ParagraphLayoutState;
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            Some(self.id.clone())
        }

        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            _cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            let line_height = px(self.font_size * self.line_height_multiplier);
            let lines_holder: Rc<RefCell<Option<Arc<Vec<WrappedLineData>>>>> =
                Rc::new(RefCell::new(None));

            let layout_id = window.request_measured_layout(Default::default(), {
                let text = self.text.clone();
                let spans = self.spans.clone();
                let family = self.family.clone();
                let font_size = self.font_size;
                let base_weight = self.base_weight;
                let bold_weight = self.bold_weight;
                let color = self.color;
                let renderer = self.renderer.clone();
                let lines_holder = lines_holder.clone();

                move |known_dimensions, available_space, window, _cx| {
                    let wrap_width = known_dimensions
                        .width
                        .or(match available_space.width {
                            gpui::AvailableSpace::Definite(w) => Some(w),
                            _ => None,
                        })
                        .map(f32::from)
                        .unwrap_or(10_000.0);

                    let scale = window.scale_factor();
                    let mut r = renderer.borrow_mut();
                    r.sync_config(family.as_deref(), font_size, base_weight, color, scale);
                    let (lines, max_width) = r.layout_paragraph(
                        &text,
                        &spans,
                        family.as_deref(),
                        font_size,
                        base_weight,
                        bold_weight,
                        wrap_width,
                    );
                    let height = px(lines.len().max(1) as f32 * f32::from(line_height));
                    let width = px(max_width.ceil());
                    *lines_holder.borrow_mut() = Some(lines);
                    size(width, height)
                }
            });

            (
                layout_id,
                ParagraphLayoutState {
                    lines: lines_holder,
                    line_height,
                },
            )
        }

        fn prepaint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut Self::RequestLayoutState,
            _window: &mut Window,
            _cx: &mut App,
        ) {
        }

        fn paint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            request_layout: &mut Self::RequestLayoutState,
            _prepaint: &mut Self::PrepaintState,
            window: &mut Window,
            _cx: &mut App,
        ) {
            let scale = window.scale_factor();
            let viewport_h = f32::from(window.viewport_size().height);
            let line_h = f32::from(request_layout.line_height);
            let origin_x_dp = f32::from(bounds.origin.x) * scale;
            let origin_y_pt = f32::from(bounds.origin.y);

            let mut renderer = self.renderer.borrow_mut();
            renderer.sync_config(
                self.family.as_deref(),
                self.font_size,
                self.base_weight,
                self.color,
                scale,
            );
            renderer.flush_drops(window);

            let lines = request_layout
                .lines
                .borrow()
                .clone()
                .unwrap_or_else(|| {
                    renderer
                        .layout_paragraph(
                            &self.text,
                            &self.spans,
                            self.family.as_deref(),
                            self.font_size,
                            self.base_weight,
                            self.bold_weight,
                            f32::from(bounds.size.width).max(1.0),
                        )
                        .0
                });

            for (line_ix, line) in lines.iter().enumerate() {
                let line_top_pt = origin_y_pt + line_ix as f32 * line_h;
                if line_top_pt + line_h < -line_h || line_top_pt > viewport_h + line_h {
                    continue;
                }

                let padding_top = (line_h - line.ascent - line.descent) * 0.5;
                let baseline_y_dp = (line_top_pt + padding_top + line.ascent) * scale;

                for run in &line.runs {
                    for (&glyph, &pos) in run.glyphs.iter().zip(run.positions.iter()) {
                        let gx_dp = origin_x_dp + pos.x as f32 * scale;
                        let gy_dp = baseline_y_dp - pos.y as f32 * scale;
                        let (int_x_dp, subpixel_x) = quantize_subpixel_x(gx_dp);
                        let int_y_dp = gy_dp.round() as i32;

                        let Some(cached) =
                            renderer.rasterize_or_get_glyph(run, glyph, subpixel_x, scale)
                        else {
                            continue;
                        };
                        let Some(img) = cached.image else {
                            continue;
                        };

                        let tile_bounds = Bounds {
                            origin: point(
                                px((int_x_dp + cached.offset_dp.x) as f32 / scale),
                                px((int_y_dp + cached.offset_dp.y) as f32 / scale),
                            ),
                            size: size(
                                px(cached.size_dp.width as f32 / scale),
                                px(cached.size_dp.height as f32 / scale),
                            ),
                        };
                        let _ = window.paint_image(
                            tile_bounds,
                            tile_bounds,
                            Corners::default(),
                            img,
                            0,
                            false,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{SmoothParagraph, SmoothTextRenderer};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_lut_is_monotonic_and_boosts_light_midtones() {
        let lut = build_alpha_lut(0.97, 0.98, 0.98, 1.0);
        assert_eq!(lut[0], 0);
        assert_eq!(lut[255], 255);
        for w in lut.windows(2) {
            assert!(w[0] <= w[1]);
        }
        // White-on-dark gamma correction lifts 50% geometric coverage into ~0.68 sRGB.
        assert!(lut[128] > 160, "expected midtone lift, got {}", lut[128]);
    }

    #[test]
    fn subpixel_quantization_wraps_cleanly() {
        assert_eq!(quantize_subpixel_x(10.0), (10, 0));
        assert_eq!(quantize_subpixel_x(10.24), (10, 1));
        assert_eq!(quantize_subpixel_x(10.9), (11, 0));
        assert_eq!(quantize_subpixel_x(-0.26), (-1, 3));
    }

    #[test]
    fn utf8_spans_map_to_utf16_around_multibyte_chars() {
        // '·' is 2 bytes UTF-8 / 1 unit UTF-16; emoji is 4 bytes UTF-8 / 2 units UTF-16.
        let s = "a·b🎙c";
        // byte indices: a=0..1, ·=1..3, b=3..4, 🎙=4..8, c=8..9
        let mapped = utf8_spans_to_utf16(s, &[(3, 4, true, false), (8, 9, false, true)]);
        // UTF-16 indices: a=0, ·=1, b=2..3, 🎙=3..5, c=5..6
        assert_eq!(mapped, vec![(2, 1, true, false), (5, 1, false, true)]);
    }

    #[test]
    fn weight_anchors_are_monotonic() {
        let weights = [300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0];
        for w in weights.windows(2) {
            assert!(weight_to_ct_trait(w[0]) < weight_to_ct_trait(w[1]));
        }
    }
}
