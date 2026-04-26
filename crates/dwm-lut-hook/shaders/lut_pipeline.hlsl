Texture2D back_buffer : register(t0);
Texture3D lut_texture : register(t1);
SamplerState point_sampler : register(s0);
cbuffer LutConstants : register(b0) {
    uint lut_size;
    uint hdr;
    float2 padding;
    float4 domain_min;
    float4 domain_max;
};

struct VsInput { float2 position : POSITION; float2 texcoord : TEXCOORD; };
struct VsOutput { float4 position : SV_POSITION; float2 texcoord : TEXCOORD; };

VsOutput VS(VsInput input) {
    VsOutput output;
    output.position = float4(input.position, 0.0, 1.0);
    output.texcoord = input.texcoord;
    return output;
}

float3 SampleLut(float3 index) {
    return lut_texture.Sample(point_sampler, (index + 0.5) / lut_size).rgb;
}

float NormalizeAxis(float value, float min_value, float max_value) {
    float range = max_value - min_value;
    if (abs(range) <= 1.1920929e-7) {
        return 0.0;
    }

    return saturate((value - min_value) / range);
}

float3 NormalizeSample(float3 rgb) {
    return float3(
        NormalizeAxis(rgb.x, domain_min.x, domain_max.x),
        NormalizeAxis(rgb.y, domain_min.y, domain_max.y),
        NormalizeAxis(rgb.z, domain_min.z, domain_max.z)
    );
}

static const float blue_noise_64x64[4096] = {
__BLUE_NOISE_64X64__
};

float BlueNoiseThreshold(float2 position) {
    uint2 pixel = uint2(position);
    uint index = ((pixel.y & 63u) * 64u) + (pixel.x & 63u);
    return blue_noise_64x64[index];
}

float3 ApplySdrDither(float3 rgb, float2 position) {
    float3 low = floor(saturate(rgb) * 255.0) / 255.0;
    float3 high = min(low + (1.0 / 255.0), 1.0);
    float threshold = BlueNoiseThreshold(position);
    float3 rgb_linear = pow(saturate(rgb), 2.2);
    float3 low_linear = pow(low, 2.2);
    float3 high_linear = pow(high, 2.2);
    float3 cutoff = lerp(low_linear, high_linear, threshold);

    return float3(
        rgb_linear.x > cutoff.x ? high.x : low.x,
        rgb_linear.y > cutoff.y ? high.y : low.y,
        rgb_linear.z > cutoff.z ? high.z : low.z
    );
}

float LinearToPq(float linear) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 32.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 128.0;
    const float c3 = 2392.0 / 128.0;
    float powered = pow(saturate(linear), m1);
    return pow((c1 + c2 * powered) / (1.0 + c3 * powered), m2);
}

float PqToLinear(float pq) {
    const float m1 = 2610.0 / 16384.0;
    const float m2 = 2523.0 / 32.0;
    const float c1 = 3424.0 / 4096.0;
    const float c2 = 2413.0 / 128.0;
    const float c3 = 2392.0 / 128.0;
    float powered = pow(saturate(pq), 1.0 / m2);
    float numerator = max(powered - c1, 0.0);
    float denominator = c2 - c3 * powered;
    return denominator <= 0.0 ? 1.0 : pow(numerator / denominator, 1.0 / m1);
}

float3 ScrgbToPq(float3 rgb) {
    const float3x3 scrgb_to_bt2100 = {
        2939026994.0 / 585553224375.0, 9255011753.0 / 3513319346250.0, 173911579.0 / 501902763750.0,
        76515593.0 / 138420033750.0, 6109575001.0 / 830520202500.0, 75493061.0 / 830520202500.0,
        12225392.0 / 93230009375.0, 1772384008.0 / 2517210253125.0, 18035212433.0 / 2517210253125.0
    };
    float3 bt2100 = mul(scrgb_to_bt2100, rgb);
    return float3(
        LinearToPq(bt2100.x),
        LinearToPq(bt2100.y),
        LinearToPq(bt2100.z)
    );
}

float3 PqToScrgb(float3 rgb) {
    const float3x3 bt2100_to_scrgb = {
        348196442125.0 / 1677558947.0, -123225331250.0 / 1677558947.0, -15276242500.0 / 1677558947.0,
        -579752563250.0 / 37238079773.0, 5273377093000.0 / 37238079773.0, -38864558125.0 / 37238079773.0,
        -12183628000.0 / 5369968309.0, -472592308000.0 / 37589778163.0, 5256599974375.0 / 37589778163.0
    };
    float3 bt2100 = float3(
        PqToLinear(rgb.x),
        PqToLinear(rgb.y),
        PqToLinear(rgb.z)
    );
    return mul(bt2100_to_scrgb, bt2100);
}

float3 Tetrahedral(float3 rgb) {
    float3 lut_index = NormalizeSample(rgb) * (lut_size - 1);
    float3 base = floor(lut_index);
    float3 frac = lut_index - base;
    float3 c000 = SampleLut(base);
    float3 c100 = SampleLut(base + float3(1, 0, 0));
    float3 c010 = SampleLut(base + float3(0, 1, 0));
    float3 c001 = SampleLut(base + float3(0, 0, 1));
    float3 c110 = SampleLut(base + float3(1, 1, 0));
    float3 c101 = SampleLut(base + float3(1, 0, 1));
    float3 c011 = SampleLut(base + float3(0, 1, 1));
    float3 c111 = SampleLut(base + 1.0);

    if (frac.x >= frac.y) {
        if (frac.y >= frac.z) return c000 + frac.x * (c100 - c000) + frac.y * (c110 - c100) + frac.z * (c111 - c110);
        if (frac.x >= frac.z) return c000 + frac.x * (c100 - c000) + frac.z * (c101 - c100) + frac.y * (c111 - c101);
        return c000 + frac.z * (c001 - c000) + frac.x * (c101 - c001) + frac.y * (c111 - c101);
    }

    if (frac.z >= frac.y) return c000 + frac.z * (c001 - c000) + frac.y * (c011 - c001) + frac.x * (c111 - c011);
    if (frac.z >= frac.x) return c000 + frac.y * (c010 - c000) + frac.z * (c011 - c010) + frac.x * (c111 - c011);
    return c000 + frac.y * (c010 - c000) + frac.x * (c110 - c010) + frac.z * (c111 - c110);
}

float4 PS(VsOutput input) : SV_TARGET {
    float3 sample = back_buffer.Sample(point_sampler, input.texcoord).rgb;
    float3 lut_input = hdr != 0 ? ScrgbToPq(sample) : sample;
    float3 lut_rgb = Tetrahedral(lut_input);
    if (hdr != 0) {
        return float4(PqToScrgb(lut_rgb), 1.0);
    }

    return float4(ApplySdrDither(lut_rgb, input.position.xy), 1.0);
}
