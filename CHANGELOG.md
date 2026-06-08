# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
