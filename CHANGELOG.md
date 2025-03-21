# Rayhunter Change Log

## [Unreleased]

### Added
- Simple build script `build-and-deploy.sh` that handles the entire build and deployment process
- New display options for the UI:
  - Full background coloring based on status (`full_background_color` option)
  - Option to hide detailed overlay (`show_screen_overlay` option)
  - Option to disable animations (`enable_animation` option)

### Changed
- Simplified build and deployment process with a single command
- Updated README.md with documentation for the configuration options
- Improved build process with automatic Docker/native detection
- Colorized terminal output for better readability

### Removed
- Dependency on multiple separate scripts for different deployment scenarios
- Manual steps for common build and deployment tasks

## [Legacy Versions]

For changes prior to this repository fork, please refer to the [Original Rayhunter Repository](https://github.com/EFForg/rayhunter/). 