# rusty-pixel

Image proxy that applies image transformations to the given image.

## Transformations

### Options

To generate an image with ratio 40/30 and with 10% margin that is resize to 200 height before

`<proxy_base_url>/scale/rh200-s30x40-m10/<url>`

- `GET` `/scale/` - Scale mode
  - `s<a size>x<b size>` - Scale with ratio
  - `r<w|h><pixels>` - Resize image to max `w` (width) or max `h` (height)
  - `m<percentage>` - Add margin from percentage base on original size, this makes the image bigger
  - `o<portrait|landscape>` - Force orientation of the image
  - `bw` - Black and white

> Resize is performed after all options.

Examples

- `s40x30` - **Scale by 40 / 30**
- `rw1000` - **Resize image to width 1000**
- Scale with margin
  - `s40x30-m10` - **Scale by 40 / 30 with added percentage margin of the shortest side**

## Configuration

Copy `config.example.toml` to `config.toml`. Every value can be overridden with
an environment variable prefixed `RP__`, using a double underscore between
nesting levels, for example `RP__APP__API_KEY` or
`RP__STORAGE__S3__SECRET_ACCESS_KEY`. Leave the S3 credentials unset to use the
AWS default credential chain.

Settings that affect throughput:

- `worker_threads`, the number of images processed at the same time (defaults to the CPU count)
- `vips_concurrency`, libvips threads per operation. Keep `worker_threads * vips_concurrency` close to the CPU count
- `upload_concurrency`, uploads in flight per process request
- `max_output_dimension`, largest side any output may have, requests above it are rejected
- `request_timeout_secs`, after which the request is abandoned and any remaining work is cancelled

## Operations

The metrics listener (`metrics_listen`) serves:

- `/metrics`, Prometheus metrics including `http_requests_duration_seconds`,
  `image_process_duration_seconds`, `image_decode_duration_seconds`,
  `image_encode_duration_seconds{format}`, `image_output_bytes{format}`,
  `image_upload_duration_seconds` and `image_queue_wait_seconds`
- `/healthz`, liveness

Every response carries an `x-request-id` header which is also included in the
logs for that request.

### Benchmark

`cargo test --release --test bench -- --ignored --nocapture` posts a 4000px
JPEG with eight configurations and prints p50 and p95 wall time.

## Contributing

### Pull Request Process

1. Ensure any install or build dependencies are not in version control.
2. Update the README.md with details of changes to the interface, this includes new environment variables, exposed ports, useful file locations and container
   parameters.
3. You may merge Pull Requests.
4. Delete branch after merge.
