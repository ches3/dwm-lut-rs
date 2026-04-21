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
    float3 lut_rgb = Tetrahedral(sample);
    return float4(ApplySdrDither(lut_rgb, input.position.xy), 1.0);
}
