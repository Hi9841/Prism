float color_brightness(float3 color) {
    // REC. 601 luminance coefficients for perceived brightness
    return dot(color, float3(0.30f, 0.59f, 0.11f));
}

float light_on_dark_contrast(float enhancedContrast, float3 color) {
    float brightness = color_brightness(color);
    float multiplier = saturate(4.0f * (0.75f - brightness));
    return enhancedContrast * multiplier;
}

float enhance_contrast(float alpha, float k) {
    return alpha * (k + 1.0f) / (alpha * k + 1.0f);
}

float apply_alpha_correction(float a, float b, float4 g) {
    float brightness_adjustment = g.x * b + g.y;
    float correction = brightness_adjustment * a + (g.z * b + g.w);
    return a + a * (1.0f - a) * correction;
}

float apply_contrast_and_gamma_correction(float sample, float3 color, float enhanced_contrast_factor, float4 gamma_ratios) {
    float enhanced_contrast = light_on_dark_contrast(enhanced_contrast_factor, color);
    float brightness = color_brightness(color);

    float contrasted = enhance_contrast(sample, enhanced_contrast);
    return apply_alpha_correction(contrasted, brightness, gamma_ratios);
}

// Skia's mask gamma "preblend" for an A8 glyph mask, as Chromium applies it on
// Windows (src/core/SkMaskGamma.cpp, SkTMaskGamma_build_correcting_lut).
// Chromium builds Skia with SK_GAMMA_SRGB and SK_GAMMA_CONTRAST=1.0 on Windows
// (skia/BUILD.gn); it reads DirectWrite values only when the user has tuned
// ClearType (ui/gfx/font_util_win.cc).
static const float SKIA_TEXT_CONTRAST = 1.0f;

float skia_srgb_to_luma(float v) {
    return v <= 0.04045f ? v / 12.92f : pow((v + 0.055f) / 1.055f, 2.4f);
}

float skia_luma_to_srgb(float v) {
    return v <= 0.0031308f ? v * 12.92f : 1.055f * pow(v, 1.0f / 2.4f) - 0.055f;
}

// The part of the preblend that is constant for one text color. The vertex
// shader computes it once for each sprite: (src, dst, lin_src, lin_dst).
float4 skia_mask_preblend_values(float3 color) {
    // The table index: the luminance of the text color in 8-bit sRGB
    // (SkComputeLuminance), cut to 3 bits (SkMaskGamma = SkTMaskGamma<3, 3, 3>).
    uint3 c = (uint3)round(saturate(color) * 255.0f);
    uint index = ((c.r * 54u + c.g * 183u + c.b * 19u) >> 8) >> 5;
    // The table's source luminance (sk_t_scale255<3>).
    float src = float((index << 5) | (index << 2) | (index >> 1)) / 255.0f;
    float dst = 1.0f - src;
    return float4(src, dst, skia_srgb_to_luma(src), skia_srgb_to_luma(dst));
}

float skia_mask_preblend(float sample, float4 values, float contrast) {
    float src = values.x;
    float dst = values.y;
    float lin_src = values.z;
    float lin_dst = values.w;
    float adjusted_contrast = contrast * lin_dst;

    float raw = round(sample * 255.0f) / 255.0f;
    float srca = raw + (1.0f - raw) * adjusted_contrast * raw;
    float result = srca;
    if (abs(src - dst) >= 1.0f / 256.0f) {
        float lin_out = lin_src * srca + (1.0f - srca) * lin_dst;
        result = (skia_luma_to_srgb(lin_out) - dst) / (src - dst);
    }
    return round(saturate(result) * 255.0f) / 255.0f;
}
