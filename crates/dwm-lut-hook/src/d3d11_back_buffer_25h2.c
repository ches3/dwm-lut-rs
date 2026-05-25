#include <stdint.h>

typedef int32_t HRESULT;

typedef struct DwmLutGuid {
    uint32_t data1;
    uint16_t data2;
    uint16_t data3;
    uint8_t data4[8];
} DwmLutGuid;

typedef struct DwmLutBackBuffer25H2Diagnostic {
    uint32_t stage;
    HRESULT hresult;
    void *container;
    void *resource;
    void *texture;
} DwmLutBackBuffer25H2Diagnostic;

enum {
    DWM_LUT_BACK_BUFFER_STAGE_ENTRY = 1,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_OVERLAY_SWAP_CHAIN = 2,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_OVERLAY_VTBL = 3,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_CONTAINER = 4,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_CONTAINER_VTBL = 5,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_RESOURCE = 6,
    DWM_LUT_BACK_BUFFER_STAGE_NULL_RESOURCE_VTBL = 7,
    DWM_LUT_BACK_BUFFER_STAGE_QUERY_INTERFACE_FAILED = 8,
    DWM_LUT_BACK_BUFFER_STAGE_SUCCESS = 9,
    DWM_LUT_BACK_BUFFER_STAGE_EXCEPTION = 10,
};

void *dwm_lut_get_back_buffer_25h2_diagnostic(
    void *overlay_swap_chain,
    uintptr_t container_vtable_index,
    uintptr_t resource_vtable_index,
    DwmLutBackBuffer25H2Diagnostic *diagnostic);

static const DwmLutGuid IID_ID3D11_TEXTURE2D = {
    0x6f15aaf2,
    0xd208,
    0x4e89,
    {0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c},
};

void *dwm_lut_get_back_buffer_25h2(
    void *overlay_swap_chain,
    uintptr_t container_vtable_index,
    uintptr_t resource_vtable_index) {
    __try {
        if (!overlay_swap_chain) {
            return 0;
        }

        void **vtbl = *(void ***)overlay_swap_chain;
        if (!vtbl || !vtbl[container_vtable_index]) {
            return 0;
        }

        void *(__stdcall *get_container)(void *) =
            (void *(__stdcall *)(void *))vtbl[container_vtable_index];
        void *container = get_container(overlay_swap_chain);
        if (!container) {
            return 0;
        }

        void **container_vtbl = *(void ***)container;
        if (!container_vtbl || !container_vtbl[resource_vtable_index]) {
            return 0;
        }

        void *(__stdcall *get_resource)(void *) =
            (void *(__stdcall *)(void *))container_vtbl[resource_vtable_index];
        void *resource = get_resource(container);
        if (!resource) {
            return 0;
        }

        void **resource_vtbl = *(void ***)resource;
        if (!resource_vtbl || !resource_vtbl[0]) {
            return 0;
        }

        HRESULT (__stdcall *query_interface)(void *, const DwmLutGuid *, void **) =
            (HRESULT (__stdcall *)(void *, const DwmLutGuid *, void **))resource_vtbl[0];
        void *texture = 0;
        if (query_interface(resource, &IID_ID3D11_TEXTURE2D, &texture) < 0 || !texture) {
            return 0;
        }

        return texture;
    } __except (1) {
        return 0;
    }
}

void *dwm_lut_get_back_buffer_25h2_diagnostic(
    void *overlay_swap_chain,
    uintptr_t container_vtable_index,
    uintptr_t resource_vtable_index,
    DwmLutBackBuffer25H2Diagnostic *diagnostic) {
    if (diagnostic) {
        diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_ENTRY;
        diagnostic->hresult = 0;
        diagnostic->container = 0;
        diagnostic->resource = 0;
        diagnostic->texture = 0;
    }

    __try {
        if (!overlay_swap_chain) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_OVERLAY_SWAP_CHAIN;
            return 0;
        }

        void **vtbl = *(void ***)overlay_swap_chain;
        if (!vtbl || !vtbl[container_vtable_index]) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_OVERLAY_VTBL;
            return 0;
        }

        void *(__stdcall *get_container)(void *) =
            (void *(__stdcall *)(void *))vtbl[container_vtable_index];
        void *container = get_container(overlay_swap_chain);
        if (diagnostic) diagnostic->container = container;
        if (!container) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_CONTAINER;
            return 0;
        }

        void **container_vtbl = *(void ***)container;
        if (!container_vtbl || !container_vtbl[resource_vtable_index]) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_CONTAINER_VTBL;
            return 0;
        }

        void *(__stdcall *get_resource)(void *) =
            (void *(__stdcall *)(void *))container_vtbl[resource_vtable_index];
        void *resource = get_resource(container);
        if (diagnostic) diagnostic->resource = resource;
        if (!resource) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_RESOURCE;
            return 0;
        }

        void **resource_vtbl = *(void ***)resource;
        if (!resource_vtbl || !resource_vtbl[0]) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_NULL_RESOURCE_VTBL;
            return 0;
        }

        HRESULT (__stdcall *query_interface)(void *, const DwmLutGuid *, void **) =
            (HRESULT (__stdcall *)(void *, const DwmLutGuid *, void **))resource_vtbl[0];
        void *texture = 0;
        HRESULT hr = query_interface(resource, &IID_ID3D11_TEXTURE2D, &texture);
        if (diagnostic) {
            diagnostic->hresult = hr;
            diagnostic->texture = texture;
        }
        if (hr < 0 || !texture) {
            if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_QUERY_INTERFACE_FAILED;
            return 0;
        }

        if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_SUCCESS;
        return texture;
    } __except (1) {
        if (diagnostic) diagnostic->stage = DWM_LUT_BACK_BUFFER_STAGE_EXCEPTION;
        return 0;
    }
}
