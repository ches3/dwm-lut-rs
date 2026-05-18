#include <stdint.h>

typedef int32_t HRESULT;

typedef struct DwmLutGuid {
    uint32_t data1;
    uint16_t data2;
    uint16_t data3;
    uint8_t data4[8];
} DwmLutGuid;

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
