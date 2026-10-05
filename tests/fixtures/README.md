These 32×32 fixtures contain FFmpeg's generated `testsrc` pattern, with no account or message data.
They exercise full JPEG/PNG/GIF/WebP decoding and JPEG `.dat` encryption in regression tests.
Generate each format with:

```sh
ffmpeg -f lavfi -i 'testsrc=size=32x32:rate=1' -frames:v 1 -threads 1 synthetic.<extension>
```
