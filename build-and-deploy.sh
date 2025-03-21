#!/bin/bash

# build-and-deploy.sh - Simple build and deployment script for Rayhunter
# This script replaces the multiple scripts in the project with a single, simple approach

set -e

# Configuration variables
TARGET_ARCH="armv7-unknown-linux-gnueabihf"
DEBUG_MODE=${DEBUG_MODE:-false}  # Set to true for verbose output
SKIP_BUILD=${SKIP_BUILD:-false}  # Set to true to skip building

# Colors for better readability
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Helper function for styled console output
print_header() {
    echo -e "\n${BLUE}===== $1 =====${NC}"
}

print_success() {
    echo -e "${GREEN}$1${NC}"
}

print_warning() {
    echo -e "${YELLOW}$1${NC}"
}

print_error() {
    echo -e "${RED}$1${NC}"
}

print_debug() {
    if [ "$DEBUG_MODE" = true ]; then
        echo -e "${YELLOW}[DEBUG] $1${NC}"
    fi
}

# Helper functions for ADB and AT commands
setup_adb() {
    if ! command -v adb &> /dev/null; then
        print_header "Setting up ADB"
        
        # Determine OS type for correct platform tools
        if [[ "$OSTYPE" == "linux-gnu"* ]]; then
            PLATFORM_TOOLS_ZIP="platform-tools-latest-linux.zip"
        elif [[ "$OSTYPE" == "darwin"* ]]; then
            PLATFORM_TOOLS_ZIP="platform-tools-latest-darwin.zip"
        else
            print_error "Unsupported operating system: $OSTYPE"
            exit 1
        fi
        
        if [ ! -d ./platform-tools ]; then
            echo "ADB not found, downloading local copy..."
            curl -O "https://dl.google.com/android/repository/$PLATFORM_TOOLS_ZIP"
            unzip "$PLATFORM_TOOLS_ZIP"
        fi
        export ADB="./platform-tools/adb"
    else
        export ADB=$(which adb)
    fi
    
    echo "Using ADB: $ADB"
}

# Check for serial tool
setup_serial() {
    print_header "Setting up Serial Tool"
    
    # Determine the correct serial binary path based on OS
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        SERIAL_PATH="./serial-ubuntu-latest/serial"
        print_debug "Checking Linux serial path: $SERIAL_PATH"
        # Try to find a built serial binary
        if [ ! -x "$SERIAL_PATH" ]; then
            SERIAL_PATH="./target/${TARGET_ARCH}/release/serial"
            print_debug "Checking alternative Linux serial path: $SERIAL_PATH"
        fi
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        # For macOS, prioritize native builds over cross-compiled ones
        SERIAL_PATH="./serial-macos-latest/serial"
        print_debug "Checking macOS serial path: $SERIAL_PATH"
        # Check in native release directory first
        if [ ! -x "$SERIAL_PATH" ]; then
            SERIAL_PATH="./target/release/serial"
            print_debug "Checking native macOS release serial path: $SERIAL_PATH"
        fi
        # Only as a last resort, try the cross-compiled version
        if [ ! -x "$SERIAL_PATH" ]; then
            SERIAL_PATH="./target/${TARGET_ARCH}/release/serial"
            print_debug "Checking cross-compiled serial path: $SERIAL_PATH"
        fi
    else
        print_error "Unsupported operating system: $OSTYPE"
        exit 1
    fi

    if [ ! -x "$SERIAL_PATH" ]; then
        print_warning "Serial binary not found at $SERIAL_PATH. Attempting to build from source..."
        
        if [ -d "./serial/src" ]; then
            print_header "Building Serial Tool"
            
            # Check if we're in a cross-compilation environment or local
            if [[ "$OSTYPE" == "darwin"* ]]; then
                # For macOS, build natively 
                print_debug "Building native macOS serial tool"
                (cd ./serial && cargo build --release)
                SERIAL_PATH="./serial/target/release/serial"
            else
                # For other platforms, use the target architecture
                print_debug "Building cross-compiled serial tool for $TARGET_ARCH"
                (cd ./serial && cargo build --release --target=${TARGET_ARCH})
                SERIAL_PATH="./serial/target/${TARGET_ARCH}/release/serial"
            fi
            
            if [ ! -x "$SERIAL_PATH" ]; then
                print_warning "Failed to build serial tool."
                export SERIAL_AVAILABLE=false
            else
                print_success "Serial tool built successfully!"
                export SERIAL_AVAILABLE=true
            fi
        else
            print_warning "Warning: Serial source code not found in ./serial/src"
            export SERIAL_AVAILABLE=false
        fi
    else
        print_success "Found serial tool at: $SERIAL_PATH"
        print_debug "Testing serial tool..."
        "$SERIAL_PATH" --help 2>&1 | head -n 1 || print_warning "Serial tool test failed"
        export SERIAL_AVAILABLE=true
        
        # On macOS, check and remove quarantine attribute if needed
        if [[ "$OSTYPE" == "darwin"* ]]; then
            if xattr "$SERIAL_PATH" 2>/dev/null | grep -q "com.apple.quarantine"; then
                print_warning "Removing quarantine attribute from serial binary..."
                xattr -d com.apple.quarantine "$SERIAL_PATH"
                print_success "Quarantine attribute removed from serial binary."
            fi
        fi
    fi
    
    print_debug "Final SERIAL_PATH: $SERIAL_PATH"
    print_debug "SERIAL_AVAILABLE: $SERIAL_AVAILABLE"
    export SERIAL_PATH="$SERIAL_PATH"
}

_adb_push() {
    echo "Pushing $1 to $2"
    "$ADB" push "$1" "$2"
}

_adb_shell() {
    "$ADB" shell "$1"
}

_at_syscmd() {
    if [ "$SERIAL_AVAILABLE" = true ]; then
        echo "Running AT command: $1"
        "$SERIAL_PATH" "AT+SYSCMD=$1"
    else
        print_warning "Cannot run AT command: $1 (serial tool not available)"
        # Fall back to using ADB and rootshell if available
        _adb_shell "/bin/rootshell -c \"$1\"" || true
    fi
}

wait_for_adb_shell() {
    echo -n "Waiting for device to be available..."
    until _adb_shell true 2> /dev/null
    do
        sleep 1
        echo -n "."
    done
    echo " ready!"
}

wait_for_atfwd_daemon() {
    echo -n "Waiting for atfwd_daemon to startup..."
    until [ -n "$(_adb_shell 'pgrep atfwd_daemon')" ]
    do
        sleep 1
        echo -n "."
    done
    echo " done!"
}

force_debug_mode() {
    if [ "$SERIAL_AVAILABLE" = true ]; then
        print_header "Enabling Debug Mode"
        echo "Force switching device into the debug mode to enable ADB..."
        print_debug "Running: $SERIAL_PATH --root"
        "$SERIAL_PATH" --root
        wait_for_adb_shell
        wait_for_atfwd_daemon
    else
        print_warning "Skipping debug mode check (serial tool not available)"
    fi
}

setup_rootshell() {
    print_header "Setting Up Rootshell"
    
    if [ -f "rootshell" ]; then
        _adb_push rootshell /tmp/
        _at_syscmd "cp /tmp/rootshell /bin/rootshell"
        sleep 1
        _at_syscmd "chown root /bin/rootshell"
        sleep 1
        _at_syscmd "chmod 4755 /bin/rootshell"
        _adb_shell '/bin/rootshell -c id'
        print_success "Root access established!"
    elif [ -f "target/${TARGET_ARCH}/release/rootshell" ]; then
        _adb_push "target/${TARGET_ARCH}/release/rootshell" /tmp/
        _at_syscmd "cp /tmp/rootshell /bin/rootshell"
        sleep 1
        _at_syscmd "chown root /bin/rootshell"
        sleep 1
        _at_syscmd "chmod 4755 /bin/rootshell"
        _adb_shell '/bin/rootshell -c id'
        print_success "Root access established!"
    else
        print_warning "Warning: rootshell binary not found, skipping rootshell setup"
        echo "If this is a first-time setup, please ensure you have the rootshell binary"
    fi
}

# Build the application
build_app() {
    print_header "Building Application"
    
    # Check if we should skip building
    if [ "$SKIP_BUILD" = true ]; then
        print_success "Skipping build as requested by SKIP_BUILD flag"
        return 0
    fi
    
    # Check if binary already exists
    TARGET_BINARY="target/${TARGET_ARCH}/release/rayhunter-daemon"
    if [ -f "$TARGET_BINARY" ]; then
        print_debug "Target binary already exists: $TARGET_BINARY"
        read -p "Binary already exists. Rebuild anyway? (y/N): " rebuild
        if [[ $rebuild != "y" && $rebuild != "Y" ]]; then
            print_success "Skipping build, using existing binary"
            return 0
        fi
    fi
    
    # Check if Docker is available AND running
    if command -v docker &> /dev/null && docker info &> /dev/null; then
        echo "Building with Docker..."
        
        # Build the Docker image
        docker build -t rayhunter-build -f Dockerfile.build .
        
        # Run the build
        mkdir -p target
        docker run --rm \
            -v "$(pwd)":/app \
            -v "$(pwd)/target":/app/target \
            -v cargo-registry:/usr/local/cargo/registry \
            -v cargo-git:/usr/local/cargo/git \
            -v cargo-target:/app/.cargo-target \
            rayhunter-build \
            /bin/bash -c "cargo build --release --target=$TARGET_ARCH"
    else
        if command -v docker &> /dev/null; then
            print_warning "Docker is installed but not running. Falling back to native build..."
        else
            echo "Docker not found. Building natively..."
        fi
        
        # Check if rustup is available
        if ! command -v rustup &> /dev/null; then
            print_error "Error: rustup not found. Please install Rust toolchain."
            exit 1
        fi
        
        # Add cross-compilation target if needed
        if ! rustup target list | grep -q "$TARGET_ARCH"; then
            echo "Adding cross-compilation target..."
            rustup target add "$TARGET_ARCH"
        fi
        
        # Check for cross-compilation toolchain
        if ! command -v arm-linux-gnueabihf-gcc &> /dev/null; then
            print_warning "Warning: Cross-compilation toolchain not found. Building may fail."
            echo "Consider installing:"
            echo "  - gcc-arm-linux-gnueabihf"
            echo "  - libc6-dev-armhf-cross"
        fi
        
        # Build the application
        cargo build --release --target="$TARGET_ARCH"
    fi
    
    print_success "Build completed successfully!"
}

# Deploy the application to the device
deploy_app() {
    print_header "Deploying to Device"
    
    echo "Creating rayhunter directory..."
    _at_syscmd "mkdir -p /data/rayhunter/qmdl"

    echo "Stopping rayhunter service if running..."
    _adb_shell '/bin/rootshell -c "/etc/init.d/rayhunter_daemon stop"' || true

    echo "Pushing configuration file..."
    if [ -f "config.toml" ]; then
        _adb_push config.toml /tmp/config.toml
    else
        echo "Configuration file not found. Creating default config..."
        cat > config.toml << EOF
qmdl_store_path = "/data/rayhunter/qmdl"
port = 8080
debug_mode = false
enable_dummy_analyzer = false
colorblind_mode = false
ui_level = 1

# UI display options:
# full_background_color = false  # When true, uses status color for entire background
# show_screen_overlay = true     # When false, shows minimal UI without detailed overlay
# enable_animation = true        # When false, disables all animations
EOF
        _adb_push config.toml /tmp/config.toml
    fi
    
    _at_syscmd "cp /tmp/config.toml /data/rayhunter/config.toml"
    _at_syscmd "chmod 644 /data/rayhunter/config.toml"

    echo "Pushing rayhunter-daemon binary..."
    _adb_push target/$TARGET_ARCH/release/rayhunter-daemon /tmp/rayhunter-daemon
    _at_syscmd "cp /tmp/rayhunter-daemon /data/rayhunter/rayhunter-daemon"
    _at_syscmd "chmod 755 /data/rayhunter/rayhunter-daemon"

    # Install service scripts
    if [ -f "./scripts/rayhunter_daemon" ]; then
        echo "Installing rayhunter service script..."
        _adb_push scripts/rayhunter_daemon /tmp/rayhunter_daemon
        _at_syscmd "cp /tmp/rayhunter_daemon /etc/init.d/rayhunter_daemon"
        _at_syscmd "chmod 755 /etc/init.d/rayhunter_daemon"
    elif [ -f "./dist/scripts/rayhunter_daemon" ]; then
        echo "Installing rayhunter service script from dist..."
        _adb_push dist/scripts/rayhunter_daemon /tmp/rayhunter_daemon
        _at_syscmd "cp /tmp/rayhunter_daemon /etc/init.d/rayhunter_daemon"
        _at_syscmd "chmod 755 /etc/init.d/rayhunter_daemon"
    else
        print_warning "Warning: rayhunter_daemon script not found, skipping"
    fi

    # Install misc service scripts if available
    if [ -f "./scripts/misc-daemon" ]; then
        echo "Installing misc-daemon service script..."
        _adb_push scripts/misc-daemon /tmp/misc-daemon
        _at_syscmd "cp /tmp/misc-daemon /etc/init.d/misc-daemon"
        _at_syscmd "chmod 755 /etc/init.d/misc-daemon"
    elif [ -f "./dist/scripts/misc-daemon" ]; then
        echo "Installing misc-daemon service script from dist..."
        _adb_push dist/scripts/misc-daemon /tmp/misc-daemon
        _at_syscmd "cp /tmp/misc-daemon /etc/init.d/misc-daemon"
        _at_syscmd "chmod 755 /etc/init.d/misc-daemon"
    fi
}

# Reboot device properly
reboot_device() {
    print_header "Rebooting Device"
    echo "Rebooting device to apply changes..."
    
    _at_syscmd "shutdown -r -t 1 now"
    
    # First wait for shutdown (it can take ~10s)
    echo -n "Waiting for device to shut down..."
    until ! _adb_shell true 2> /dev/null
    do
        sleep 1
        echo -n "."
    done
    echo " done!"
    
    echo "Device is shutting down. Waiting for it to boot back up..."
    
    # Now wait for boot to finish
    wait_for_adb_shell
    wait_for_atfwd_daemon
    
    print_success "Device rebooted successfully!"
}

# Test connection to rayhunter
test_connection() {
    print_header "Testing Rayhunter"
    
    echo "Checking if rayhunter service is running..."
    if ! _adb_shell "/bin/rootshell -c 'ps | grep rayhunter-daemon'" | grep -q rayhunter-daemon; then
        echo "Starting rayhunter service..."
        _adb_shell '/bin/rootshell -c "/etc/init.d/rayhunter_daemon start"'
    else
        print_success "Rayhunter service is already running."
    fi
    
    # Set up port forwarding
    PORT=8080
    
    # Check if port forwarding is already set up
    if ! "$ADB" forward --list | grep -q "tcp:$PORT"; then
        "$ADB" forward tcp:$PORT tcp:$PORT > /dev/null
        echo "Port forwarding set up: localhost:$PORT -> device:$PORT"
    else
        print_debug "Port forwarding already set up for port $PORT"
        echo "Using existing port forwarding: localhost:$PORT -> device:$PORT"
    fi
    
    # Test connection
    URL="http://localhost:$PORT"
    echo -n "Testing connection to rayhunter server..."
    
    SECONDS=0
    while (( SECONDS < 30 )); do
        if curl -L --fail "$URL" -o /dev/null -s; then
            echo " success!"
            print_success "You can access rayhunter at $URL"
            return 0
        fi
        sleep 1
        echo -n "."
    done
    
    print_warning "Timeout reached! Failed to reach rayhunter URL."
    echo ""
    echo "To see the log file, run:"
    echo "adb shell \"/bin/rootshell -c 'cat /data/rayhunter/rayhunter.log'\""
    return 1
}

# Main script logic
print_header "Rayhunter Build & Deploy"
echo "This script will build and deploy Rayhunter to your device."
print_debug "Debug mode is ENABLED. To disable, run with: DEBUG_MODE=false $0"
if [ "$DEBUG_MODE" = false ]; then
    echo "For verbose output, run with: DEBUG_MODE=true $0"
fi
if [ "$SKIP_BUILD" = true ]; then
    print_success "Build will be skipped (SKIP_BUILD=true)"
else
    echo "To skip building, run with: SKIP_BUILD=true $0"
fi

# Setup tools
setup_adb
setup_serial

# Enable debug mode
force_debug_mode

# Set up root access
setup_rootshell

# Build and deploy
build_app
deploy_app

# Reboot device and wait for it to come back
reboot_device

# Test the connection
test_connection

print_success "Operation completed successfully!"
echo "You can edit config.toml to customize Rayhunter settings and run this script again to deploy the changes."
exit 0 