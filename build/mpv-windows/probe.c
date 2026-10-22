// Loads the built DLLs and prints what check.py verifies: the licence each FFmpeg library reports, whether mpv accepts the
// options the Windows player uses (ADR 0007), and the decoders and demuxers mpv sees. Exits non-zero if an option is rejected.
#include <stdio.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavfilter/avfilter.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libswresample/swresample.h>
#include <libswscale/swscale.h>
#include <mpv/client.h>

static int set(mpv_handle *mpv, const char *name, const char *value)
{
    int err = mpv_set_option_string(mpv, name, value);
    printf("option %s=%s: %s\n", name, value, mpv_error_string(err));
    return err >= 0;
}

static void print_string(mpv_handle *mpv, const char *name)
{
    char *value = mpv_get_property_string(mpv, name);
    printf("property %s: %s\n", name, value ? value : "(unavailable)");
    mpv_free(value);
}

// Prints one line per entry of a list property whose entries are maps, using the given key.
static void print_list(mpv_handle *mpv, const char *name, const char *key)
{
    mpv_node node;
    if (mpv_get_property(mpv, name, MPV_FORMAT_NODE, &node) < 0) {
        printf("property %s: (unavailable)\n", name);
        return;
    }
    if (node.format == MPV_FORMAT_NODE_ARRAY) {
        for (int i = 0; i < node.u.list->num; i++) {
            mpv_node *entry = &node.u.list->values[i];
            if (entry->format == MPV_FORMAT_STRING) {
                printf("%s: %s\n", name, entry->u.string);
                continue;
            }
            if (entry->format != MPV_FORMAT_NODE_MAP)
                continue;
            for (int k = 0; k < entry->u.list->num; k++) {
                mpv_node *v = &entry->u.list->values[k];
                if (strcmp(entry->u.list->keys[k], key) == 0 && v->format == MPV_FORMAT_STRING)
                    printf("%s: %s\n", name, v->u.string);
            }
        }
    }
    mpv_free_node_contents(&node);
}

int main(void)
{
    printf("licence avcodec: %s\n", avcodec_license());
    printf("licence avformat: %s\n", avformat_license());
    printf("licence avfilter: %s\n", avfilter_license());
    printf("licence avutil: %s\n", avutil_license());
    printf("licence swresample: %s\n", swresample_license());
    printf("licence swscale: %s\n", swscale_license());
    printf("ffmpeg configuration: %s\n", avutil_configuration());

    mpv_handle *mpv = mpv_create();
    if (!mpv) {
        printf("mpv_create failed\n");
        return 1;
    }
    printf("mpv client api: %lu.%lu\n", mpv_client_api_version() >> 16, mpv_client_api_version() & 0xffff);

    // Option values are validated when set, so an mpv without D3D11, composition mode or WASAPI rejects these.
    int ok = 1;
    ok &= set(mpv, "vo", "gpu-next");
    ok &= set(mpv, "gpu-api", "d3d11");
    ok &= set(mpv, "gpu-context", "d3d11");
    ok &= set(mpv, "d3d11-output-mode", "composition");
    ok &= set(mpv, "d3d11-composition-size", "1280x720");
    ok &= set(mpv, "display-fps-override", "60");
    ok &= set(mpv, "target-colorspace-hint", "yes");
    ok &= set(mpv, "target-colorspace-hint-mode", "target");
    ok &= set(mpv, "hdr-reference-white", "203");
    ok &= set(mpv, "hwdec", "d3d11va");
    ok &= set(mpv, "ao", "wasapi");
    ok &= set(mpv, "sub-codepage", "auto");

    // Start without video or audio output: the runner has no display to present to.
    ok &= set(mpv, "vo", "null");
    ok &= set(mpv, "ao", "null");
    ok &= set(mpv, "config", "no");
    ok &= set(mpv, "idle", "yes");
    int err = mpv_initialize(mpv);
    printf("mpv_initialize: %s\n", mpv_error_string(err));
    if (err < 0) {
        mpv_destroy(mpv);
        return 1;
    }
    print_string(mpv, "mpv-version");
    print_string(mpv, "ffmpeg-version");
    print_string(mpv, "libass-version");
    print_string(mpv, "mpv-configuration");
    print_list(mpv, "decoder-list", "codec");
    print_list(mpv, "demuxer-lavf-list", "");
    mpv_terminate_destroy(mpv);
    printf("probe: %s\n", ok ? "ok" : "an option was rejected");
    return ok ? 0 : 1;
}
