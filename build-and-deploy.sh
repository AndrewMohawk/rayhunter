#!/bin/bash

# build-and-deploy.sh - Simple build and deployment script for Rayhunter
# This script replaces the multiple scripts in the project with a single, simple approach

set -e

# Configuration variables
TARGET_ARCH="armv7-unknown-linux-gnueabihf"

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
    # Determine the correct serial binary path based on OS
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        SERIAL_PATH="./serial-ubuntu-latest/serial"
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        SERIAL_PATH="./serial-macos-latest/serial"
    else
        print_error "Unsupported operating system: $OSTYPE"
        exit 1
    fi

    if [ ! -x "$SERIAL_PATH" ]; then
        print_warning "Warning: The serial binary cannot be found at $SERIAL_PATH."
        print_warning "Download it from the latest release bundle at https://github.com/EFForg/rayhunter/releases"
        export SERIAL_AVAILABLE=false
    else
        export SERIAL_AVAILABLE=true
    fi
    
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
    
    # Check if Docker is available
    if command -v docker &> /dev/null; then
        echo "Building with Docker..."
        
        # Build the Docker image
        docker build -t rayhunter-build -f Dockerfile.build .
        
        # Run the build
        mkdir -p target
        docker run --rm \
            -v "$(pwd)":/app \
            -v "$(pwd)/target":/app/target \
            -v cargo-registry:/usr/local/cargo/registry \
            rayhunter-build \
            /bin/bash -c "cargo build --release --target=$TARGET_ARCH"
    else
        echo "Building natively..."
        
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
            print_error "Error: Cross-compilation toolchain not found. Please install:"
            echo "  - gcc-arm-linux-gnueabihf"
            echo "  - libc6-dev-armhf-cross"
            exit 1
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
    "$ADB" forward tcp:$PORT tcp:$PORT > /dev/null
    echo "Port forwarding set up: localhost:$PORT -> device:$PORT"
    
    # Test connection
    URL="http://localhost:$PORT"
    echo -n "Testing connection to rayhunter server..."
    
    SECONDS=0
    while (( SECONDS < 30 )); do
        if curl -L --fail-with-body "$URL" -o /dev/null -s; then
            echo " success!"
            print_success "You can access rayhunter at $URL"
            return 0
        fi
        sleep 1
        echo -n "."
    done
    
    print_warning "Timeout reached! Failed to reach rayhunter URL."
    echo "Check the device screen - you should see a YELLOW LINE at the top if the UI is working"
    echo ""
    echo "To see the log file, run:"
    echo "adb shell \"/bin/rootshell -c 'cat /data/rayhunter/rayhunter.log'\""
    return 1
}

# Main script logic
print_header "Rayhunter Build & Deploy"
echo "This script will build and deploy Rayhunter to your device."

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