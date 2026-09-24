/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

//! Maka's palette (DESIGN.md §2, §3, §8): a neutral ladder, two text tiers,
//! one accent and three statuses, derived from the same oklch values as the
//! Electron app.

use gpui_kit::{App, Global, Hsla, Rgba, SharedString, WindowAppearance};

pub struct Theme {
    pub dark: bool,
    /// Window floor, behind the transcript.
    pub base: Hsla,
    pub sidebar: Hsla,
    /// Reading surfaces: the transcript and the composer.
    pub raised: Hsla,
    /// Menus, tooltips, popovers.
    pub overlay: Hsla,
    /// Recessed wells: code blocks, table headers.
    pub sunken: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    /// Hover and selected rows share one wash.
    pub hover: Hsla,
    pub accent: Hsla,
    /// Accent as text or as a filled button.
    pub accent_solid: Hsla,
    pub on_accent: Hsla,
    pub selection: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub ui_font: SharedString,
    pub mono_font: SharedString,
}

impl Global for Theme {}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let pick = |light: (f32, f32, f32), dark_value: (f32, f32, f32)| {
            let (l, c, h) = if dark { dark_value } else { light };
            oklch(l, c, h, 1.)
        };
        let ink = pick((0.17, 0.005, 286.), (0.95, 0.004, 286.));
        let background = if dark { 0.205 } else { 1.0 };
        let neutral = |l: f32| oklch(l, if dark { 0.004 } else { 0. }, 286., 1.);
        let accent = pick((0.70, 0.135, 250.), (0.74, 0.15, 250.));
        Self {
            dark,
            base: neutral(background - 0.025),
            sidebar: neutral(background - if dark { 0.04 } else { 0.045 }),
            raised: neutral(background),
            overlay: neutral(if dark { background + 0.018 } else { background }),
            sunken: neutral(background - if dark { 0.065 } else { 0.055 }),
            text: ink,
            muted: mix(ink, neutral(background), 0.68),
            border: ink.opacity(0.10),
            hover: ink.opacity(0.05),
            accent,
            accent_solid: pick((0.48, 0.135, 250.), (0.76, 0.15, 250.)),
            on_accent: if dark {
                oklch(0.2, 0., 0., 1.)
            } else {
                oklch(1., 0., 0., 1.)
            },
            selection: accent.opacity(if dark { 0.40 } else { 0.28 }),
            warning: pick((0.50, 0.18, 55.), (0.66, 0.18, 55.)),
            danger: pick((0.50, 0.24, 28.), (0.70, 0.19, 22.)),
            ui_font: ".SystemUIFont".into(),
            mono_font: "Menlo".into(),
        }
    }
}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// `ink` over `ground`, `amount` of the way to ink.
fn mix(ink: Hsla, ground: Hsla, amount: f32) -> Hsla {
    let (a, b) = (Rgba::from(ink), Rgba::from(ground));
    let lerp = |x: f32, y: f32| y + (x - y) * amount;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: 1.,
    }
    .into()
}

fn oklch(l: f32, c: f32, h: f32, alpha: f32) -> Hsla {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let linear = [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ];
    let gamma = |x: f32| {
        let x = x.clamp(0., 1.);
        if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1. / 2.4) - 0.055
        }
    };
    Rgba {
        r: gamma(linear[0]),
        g: gamma(linear[1]),
        b: gamma(linear[2]),
        a: alpha,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::oklch;
    use gpui_kit::Rgba;

    #[test]
    fn oklch_matches_known_srgb() {
        let white = Rgba::from(oklch(1., 0., 0., 1.));
        assert!((white.r - 1.).abs() < 0.01 && (white.b - 1.).abs() < 0.01);
        // DESIGN.md's brand blue family: oklch(0.70 0.135 250) ≈ #5ea0f0.
        let blue = Rgba::from(oklch(0.70, 0.135, 250., 1.));
        assert!(blue.b > blue.g && blue.g > blue.r, "{blue:?}");
    }
}
