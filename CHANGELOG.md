# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.8] - 2026-09-28

### Changed

- Configurations in process image are decoded straight to their target size
  (shrink on load) and processed in parallel, uploads run concurrently
- Environment image is decoded once per request
- Output metadata is stripped and JPEG Huffman tables are optimised, outputs
  are tagged sRGB
- Black and white outputs are three band grey in sRGB
- Work is cancelled when the client disconnects or the request times out
- Scale endpoint returns 400 for invalid options and sends Cache-Control
- Output dimensions are capped by `max_output_dimension`
- Local storage rejects keys that escape the storage root
- Configuration can be overridden with `RP__` environment variables and S3
  credentials fall back to the AWS default chain
- Request ids and processing metrics
- Docker image built on Alpine 3.24 instead of edge, libvips version pinned

### Fixed

- SVG passthrough stopped processing after the first configuration
- `vips_cache_max_mem_mb` was read but never applied
- OpenAPI version now follows Cargo.toml
- Scale endpoint converts wide gamut and CMYK sources to sRGB when no resize
  step does it, instead of labelling their pixels sRGB
- Thumbnails no longer force an sRGB input profile, which broke CMYK sources
  without a profile and warned "profile incompatible with image" on black and
  white images

## [0.1.7] - 2026-06-08

### Changed

- Fixed vips binding issue for orientation by using another op
- Fixed generating alternatives when uploading SVG without allow_vector

## [0.1.6] - 2026-06-04

### Changed

- Use alpine edge for vips 8.18

## [0.1.5] - 2026-06-03

### Changed

- Added width and height to the output
- Improved error logging

## [0.1.4] - 2026-03-27

### Changed

- Update Rust edition and dependencies

## [0.1.3] - 2026-02-25

### Added

- ReDoc UI for interactive API documentation

### Changed

- Improved error handling across HTTP handlers

### Fixed

- Prevent double slashes in URL paths
- Join URL paths correctly

## [0.1.0] - 2025-06-12

### Added

- Initial release with core image proxy functionality
- Path-based transformation syntax (scale, resize, orientation, grayscale, margin, trim)
- S3 and local filesystem storage backends
- libvips integration for image processing
