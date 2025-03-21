use image::{codecs::gif::GifDecoder, imageops::FilterType, AnimationDecoder, DynamicImage};
use std::{io::Cursor, time::Duration, fs, io::Write};
use crate::config::Config;

// Version number - set to 0.0.1 by default
pub const VERSION: &str = "0.0.1";

const FB_PATH:&str = "/dev/fb0";

// Animation counter - will be incremented each time the display is updated
static mut ANIMATION_COUNTER: u32 = 0;

#[derive(Copy, Clone)]
// TODO actually poll for this, maybe w/ fbset?
struct Dimensions {
    height: u32,
    width: u32,
}

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub enum Color565 {
    Red    = 0b1111100000000000,
    Green  = 0b0000011111100000,
    Blue   = 0b0000000000011111,
    White  = 0b1111111111111111,
    Black  = 0b0000000000000000,
    Cyan   = 0b0000011111111111,
    Yellow = 0b1111111111100000,
    Pink =   0b1111010010011111,
}

#[derive(Clone)]
pub enum DisplayState {
    Recording,
    Paused,
    WarningDetected,
    RecordingCBM,
    AnalysisWarning { message: String, severity: String },
    DetailedStatus { 
        qmdl_name: String,
        qmdl_size_bytes: usize,
        analysis_size_bytes: usize,
        num_warnings: usize,
        last_warning: Option<String>,
    },
}

impl From<DisplayState> for Color565 {
    fn from(state: DisplayState) -> Self {
        match state {
            DisplayState::Paused => Color565::White,
            DisplayState::Recording => Color565::Green, 
            DisplayState::RecordingCBM => Color565::Blue, 
            DisplayState::WarningDetected => Color565::Red,
            DisplayState::AnalysisWarning { severity, .. } => {
                match severity.as_str() {
                    "High" => Color565::Red,
                    "Medium" => Color565::Yellow,
                    "Low" => Color565::Cyan,
                    _ => Color565::White,
                }
            },
            DisplayState::DetailedStatus { num_warnings, .. } => {
                if num_warnings > 0 {
                    Color565::Yellow
                } else {
                    Color565::Green
                }
            }
        }
    }
}

#[derive(Copy, Clone)]
pub struct Framebuffer<'a> {
    dimensions: Dimensions,
    path: &'a str,
}

impl Framebuffer<'_>{
    pub const fn new() -> Self {
        Framebuffer{
            dimensions: Dimensions{height: 128, width: 128},
            path: FB_PATH,
        }
    }

    fn write(&mut self, img: DynamicImage) {
        let mut width = img.width();
        let mut height = img.height();
        let resized_img: DynamicImage;
        if height > self.dimensions.height ||
        width > self.dimensions.width {
            resized_img = img.resize( self.dimensions.width, self.dimensions.height, FilterType::CatmullRom);
            width = self.dimensions.width.min(resized_img.width());
            height = self.dimensions.height.min(resized_img.height());
        } else {
            resized_img = img;
        }
        let img_rgba8 = resized_img.as_rgba8().unwrap();
        let mut buf = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let px = img_rgba8.get_pixel(x, y);
                let mut rgb565: u16 = (px[0] as u16 & 0b11111000) << 8;
                rgb565 |= (px[1] as u16 & 0b11111100) << 3;
                rgb565 |= (px[2] as u16) >> 3;
                buf.extend(rgb565.to_le_bytes());
            }
        }
        std::fs::write(self.path, &buf).unwrap();
    }

    pub fn draw_gif(&mut self, img_buffer: &[u8]) {
        // this is dumb and i'm sure there's a better way to loop this
        let cursor = Cursor::new(img_buffer);
        let decoder = GifDecoder::new(cursor).unwrap();
        for maybe_frame in decoder.into_frames() {
            let frame = maybe_frame.unwrap();
            let (numerator, _) = frame.delay().numer_denom_ms();
            let img = DynamicImage::from(frame.into_buffer());
            self.write(img);
            std::thread::sleep(Duration::from_millis(numerator as u64));
        }
    }

    pub fn draw_img(&mut self, img_buffer: &[u8]) {
        let img = image::load_from_memory(img_buffer).unwrap();
        self.write(img);
    }

    pub fn draw_line(&mut self, color: Color565, height: u32){
        let px_num= height * self.dimensions.width;
        let color: u16 = color as u16;
        let mut buffer: Vec<u8> = Vec::new();
        for _ in 0..px_num {
            buffer.extend(color.to_le_bytes());
        }
        std::fs::write(self.path, &buffer).unwrap();
    }

    pub fn draw_warning(&mut self, message: &str, severity: &str, color: Color565) {
        // First draw the color line to indicate status
        self.draw_line(color, 10);
        
        // Prepare the buffer for text - start after the color line
        let mut buffer: Vec<u8> = Vec::new();
        let color_text = Color565::White as u16;
        let color_bg = Color565::Black as u16;
        
        // Truncate message if it's too long (for screen clarity)
        let display_msg = if message.len() > 20 {
            format!("{}...", &message[0..17])
        } else {
            message.to_string()
        };
        
        // Create a simple text display - just the first 10 rows after the colored line
        // This is a very simple approach without true font rendering
        for y in 11..40 {
            for x in 0..self.dimensions.width {
                // Background color for all pixels
                let pixel_color = if y < 25 && x < display_msg.len() as u32 * 6 {
                    // For the area where text should be, use foreground color
                    color_text
                } else {
                    color_bg
                };
                buffer.extend(pixel_color.to_le_bytes());
            }
        }
        
        // Write severity info on the screen - not actually using it in this simplified version
        // A real implementation would render this text properly
        let _severity_text = format!("Severity: {}", severity);
        
        // This is a simple implementation - in a real system you would want
        // proper text rendering with fonts
        std::fs::write(self.path, &buffer).unwrap();
    }

    // Simple function to draw a digit using block rendering
    fn draw_digit(&self, buffer: &mut Vec<u8>, digit: u8, x_offset: u32, y_offset: u32) {
        let color_text = Color565::White as u16;
        let color_bg = Color565::Black as u16;
        
        // Simple 3x5 digit patterns
        let patterns = [
            // 0
            [
                true, true, true,
                true, false, true,
                true, false, true,
                true, false, true,
                true, true, true,
            ],
            // 1
            [
                false, true, false,
                true, true, false,
                false, true, false,
                false, true, false,
                true, true, true,
            ],
            // 2
            [
                true, true, true,
                false, false, true,
                true, true, true,
                true, false, false,
                true, true, true,
            ],
            // 3
            [
                true, true, true,
                false, false, true,
                false, true, true,
                false, false, true,
                true, true, true,
            ],
            // 4
            [
                true, false, true,
                true, false, true,
                true, true, true,
                false, false, true,
                false, false, true,
            ],
            // 5
            [
                true, true, true,
                true, false, false,
                true, true, true,
                false, false, true,
                true, true, true,
            ],
            // 6
            [
                true, true, true,
                true, false, false,
                true, true, true,
                true, false, true,
                true, true, true,
            ],
            // 7
            [
                true, true, true,
                false, false, true,
                false, true, false,
                true, false, false,
                true, false, false,
            ],
            // 8
            [
                true, true, true,
                true, false, true,
                true, true, true,
                true, false, true,
                true, true, true,
            ],
            // 9
            [
                true, true, true,
                true, false, true,
                true, true, true,
                false, false, true,
                true, true, true,
            ],
        ];
        
        let pattern = patterns[digit as usize];
        let digit_width = 3u32;
        let digit_height = 5u32;
        let scale = 2u32; // Scale factor to make digits larger
        
        for y in 0..digit_height {
            for x in 0..digit_width {
                let idx = (y * digit_width + x) as usize;
                let is_set = pattern[idx];
                
                // Draw a scaled pixel (2x2)
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = x_offset + x * scale + sx;
                        let py = y_offset + y * scale + sy;
                        
                        // Ensure we're within the screen bounds
                        if px < self.dimensions.width && py < self.dimensions.height {
                            let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                            if buffer_idx + 1 < buffer.len() {
                                let pixel = if is_set { color_text } else { color_bg };
                                buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Function to render a number using block digits
    fn draw_number(&self, buffer: &mut Vec<u8>, number: usize, x_offset: u32, y_offset: u32) {
        // Convert number to string and draw each digit
        let num_str = number.to_string();
        let digit_width = 8; // Width of each digit including spacing
        
        for (i, c) in num_str.chars().enumerate() {
            if let Some(digit) = c.to_digit(10) {
                self.draw_digit(buffer, digit as u8, x_offset + (i as u32 * digit_width), y_offset);
            }
        }
    }
    
    // Function to draw a simple status icon - make it larger and more visible
    fn draw_status_icon(&self, buffer: &mut Vec<u8>, icon_type: &str, x_offset: u32, y_offset: u32) {
        let color_ok = Color565::Green as u16;
        let color_warn = Color565::Yellow as u16;
        let color_error = Color565::Red as u16;
        let color_bg = Color565::Black as u16;
        
        let size = 16u32; // Increased icon size for better visibility
        
        match icon_type {
            "ok" => {
                // Draw a thicker checkmark
                for y in 0..size {
                    for x in 0..size {
                        let px = x_offset + x;
                        let py = y_offset + y;
                        
                        // Thicker checkmark pattern
                        let is_set = 
                            // Horizontal part of checkmark
                            (x >= size/3 && x <= size/2 && y >= size/2 && y <= size/2+2) ||
                            // Diagonal part of checkmark
                            (x >= size/2 && x <= size-2 && 
                             (y == size/2+2-(x-size/2) || y == size/2+3-(x-size/2)));
                        
                        if px < self.dimensions.width && py < self.dimensions.height {
                            let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                            if buffer_idx + 1 < buffer.len() {
                                let pixel = if is_set { color_ok } else { color_bg };
                                buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                            }
                        }
                    }
                }
            },
            "warning" => {
                // Define the triangle dimensions
                let top_y = 1u32;
                let bottom_y = size - 2;
                let mid_x = size / 2;
                
                // Draw a more visible warning triangle (filled with yellow)
                for y in 0..size {
                    for x in 0..size {
                        let px = x_offset + x;
                        let py = y_offset + y;
                        
                        // Calculate triangle bounds (wider triangle)
                        let left_x = mid_x - (y - top_y) / 2 - 1;
                        let right_x = mid_x + (y - top_y) / 2 + 1;
                        
                        // Fill the entire triangle
                        let is_in_triangle = y >= top_y && y <= bottom_y && x >= left_x && x <= right_x;
                        
                        // Create a solid border
                        let is_border = 
                            (y == top_y && (x == mid_x || x == mid_x - 1 || x == mid_x + 1)) ||
                            (y == bottom_y && x >= left_x && x <= right_x) ||
                            (y > top_y && y < bottom_y && (x == left_x || x == right_x));
                        
                        if px < self.dimensions.width && py < self.dimensions.height {
                            let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                            if buffer_idx + 1 < buffer.len() {
                                // Use different shade for fill vs border
                                let pixel = if is_border { 
                                    color_warn 
                                } else if is_in_triangle { 
                                    // Use a darker yellow for the fill
                                    (color_warn & 0xFFE0) | 0x0200 
                                } else { 
                                    color_bg 
                                };
                                
                                buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                            }
                        }
                    }
                }
                
                // Add a larger, more visible exclamation mark inside the triangle
                for y in 0..size-6 {
                    let px_start = x_offset + mid_x - 1;
                    let py = y_offset + top_y + 3 + y;
                    
                    // Draw the exclamation mark stem and dot
                    let is_exclamation = y < size/2 - 2 || y > size/2;
                    
                    if is_exclamation && py < self.dimensions.height {
                        // Make the exclamation mark 2 pixels wide
                        for x_offset in 0..3 {
                            let px = px_start + x_offset;
                            if px < self.dimensions.width {
                                let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                                if buffer_idx + 1 < buffer.len() {
                                    // Black exclamation mark
                                    let color = Color565::Black as u16;
                                    buffer[buffer_idx] = (color & 0xFF) as u8;
                                    buffer[buffer_idx + 1] = (color >> 8) as u8;
                                }
                            }
                        }
                    }
                }
            },
            "error" => {
                // Draw a larger, more visible X
                for y in 0..size {
                    for x in 0..size {
                        let px = x_offset + x;
                        let py = y_offset + y;
                        
                        let is_set = 
                            // First diagonal (top-left to bottom-right)
                            ((x == y || x == y+1 || x+1 == y) && x >= 2 && x <= size-3) ||
                            // Second diagonal (top-right to bottom-left)
                            ((x + y == size-1 || x + y == size || x + y == size-2) && x >= 2 && x <= size-3);
                        
                        if px < self.dimensions.width && py < self.dimensions.height {
                            let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                            if buffer_idx + 1 < buffer.len() {
                                let pixel = if is_set { color_error } else { color_bg };
                                buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                            }
                        }
                    }
                }
            },
            _ => {}
        }
    }

    // Function to draw simple text using block letters (just supports few labels)
    fn draw_label(&self, buffer: &mut Vec<u8>, label: &str, x_offset: u32, y_offset: u32) {
        let color_label = Color565::Cyan as u16;
        let color_bg = Color565::Black as u16;
        let pixel_size = 2u32; // Size of each pixel in the label
        
        // Simple patterns for a few key labels we need
        let get_pattern = |c: char| -> Vec<bool> {
            match c {
                'K' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, false,
                    true, false, true,
                    true, false, true,
                ],
                'B' => vec![
                    true, true, false,
                    true, false, true,
                    true, true, false,
                    true, false, true,
                    true, true, false,
                ],
                'S' => vec![
                    true, true, true,
                    true, false, false,
                    true, true, true,
                    false, false, true,
                    true, true, true,
                ],
                'I' => vec![
                    true, true, true,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                    true, true, true,
                ],
                'Z' => vec![
                    true, true, true,
                    false, false, true,
                    false, true, false,
                    true, false, false,
                    true, true, true,
                ],
                'E' => vec![
                    true, true, true,
                    true, false, false,
                    true, true, false,
                    true, false, false,
                    true, true, true,
                ],
                'A' => vec![
                    true, true, true,
                    true, false, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'N' => vec![
                    true, false, true,
                    true, true, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'L' => vec![
                    true, false, false,
                    true, false, false,
                    true, false, false,
                    true, false, false,
                    true, true, true,
                ],
                'Y' => vec![
                    true, false, true,
                    true, false, true,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                ],
                'W' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, true,
                    true, true, true,
                    true, false, true,
                ],
                'R' => vec![
                    true, true, false,
                    true, false, true,
                    true, true, false,
                    true, false, true,
                    true, false, true,
                ],
                'G' => vec![
                    true, true, true,
                    true, false, false,
                    true, false, true,
                    true, false, true,
                    true, true, true,
                ],
                'H' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'U' => vec![
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, true, true,
                ],
                'T' => vec![
                    true, true, true,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                ],
                'V' => vec![
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    false, true, false,
                ],
                'O' => vec![
                    true, true, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, true, true,
                ],
                'P' => vec![
                    true, true, true,
                    true, false, true,
                    true, true, true,
                    true, false, false,
                    true, false, false,
                ],
                'D' => vec![
                    true, true, false,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, true, false,
                ],
                ':' => vec![
                    false, true, false,
                    false, true, false,
                    false, false, false,
                    false, true, false,
                    false, true, false,
                ],
                '.' => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, true, false,
                ],
                '-' => vec![
                    false, false, false,
                    false, false, false,
                    true, true, true,
                    false, false, false,
                    false, false, false,
                ],
                '0' => vec![
                    true, true, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, true, true,
                ],
                '1' => vec![
                    false, true, false,
                    true, true, false,
                    false, true, false,
                    false, true, false,
                    true, true, true,
                ],
                '2' => vec![
                    true, true, true,
                    false, false, true,
                    true, true, true,
                    true, false, false,
                    true, true, true,
                ],
                '3' => vec![
                    true, true, true,
                    false, false, true,
                    false, true, true,
                    false, false, true,
                    true, true, true,
                ],
                '4' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, true,
                    false, false, true,
                    false, false, true,
                ],
                '5' => vec![
                    true, true, true,
                    true, false, false,
                    true, true, true,
                    false, false, true,
                    true, true, true,
                ],
                '6' => vec![
                    true, true, true,
                    true, false, false,
                    true, true, true,
                    true, false, true,
                    true, true, true,
                ],
                '7' => vec![
                    true, true, true,
                    false, false, true,
                    false, true, false,
                    true, false, false,
                    true, false, false,
                ],
                '8' => vec![
                    true, true, true,
                    true, false, true,
                    true, true, true,
                    true, false, true,
                    true, true, true,
                ],
                '9' => vec![
                    true, true, true,
                    true, false, true,
                    true, true, true,
                    false, false, true,
                    true, true, true,
                ],
                ' ' => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                ],
                _ => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                ],
            }
        };
        
        let char_width = 3u32;
        let char_height = 5u32;
        let char_spacing = 1u32;
        
        let mut x_pos = x_offset;
        for c in label.chars() {
            let pattern = get_pattern(c);
            
            for y in 0..char_height {
                for x in 0..char_width {
                    let idx = (y * char_width + x) as usize;
                    let is_set = idx < pattern.len() && pattern[idx];
                    
                    // Draw scaled pixels
                    for sy in 0..pixel_size {
                        for sx in 0..pixel_size {
                            let px = x_pos + x * pixel_size + sx;
                            let py = y_offset + y * pixel_size + sy;
                            
                            if px < self.dimensions.width && py < self.dimensions.height {
                                let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                                if buffer_idx + 1 < buffer.len() {
                                    let pixel = if is_set { color_label } else { color_bg };
                                    buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                    buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                                }
                            }
                        }
                    }
                }
            }
            
            x_pos += (char_width + char_spacing) * pixel_size;
        }
    }

    // Function for drawing header text with larger size and different color
    fn draw_header(&self, buffer: &mut Vec<u8>, label: &str, x_offset: u32, y_offset: u32) {
        let color_label = Color565::White as u16; // White is more visible
        let color_bg = Color565::Black as u16;
        let pixel_size = 3u32; // Larger size for header
        
        // Use the same pattern getter but with larger pixels
        let get_pattern = |c: char| -> Vec<bool> {
            match c {
                // Same patterns as draw_label
                'K' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, false,
                    true, false, true,
                    true, false, true,
                ],
                // ... and so on for other characters
                'R' => vec![
                    true, true, false,
                    true, false, true,
                    true, true, false,
                    true, false, true,
                    true, false, true,
                ],
                'A' => vec![
                    true, true, true,
                    true, false, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'Y' => vec![
                    true, false, true,
                    true, false, true,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                ],
                'H' => vec![
                    true, false, true,
                    true, false, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'U' => vec![
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, true, true,
                ],
                'N' => vec![
                    true, false, true,
                    true, true, true,
                    true, true, true,
                    true, false, true,
                    true, false, true,
                ],
                'T' => vec![
                    true, true, true,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                    false, true, false,
                ],
                'E' => vec![
                    true, true, true,
                    true, false, false,
                    true, true, false,
                    true, false, false,
                    true, true, true,
                ],
                'V' => vec![
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    true, false, true,
                    false, true, false,
                ],
                '1' => vec![
                    false, true, false,
                    true, true, false,
                    false, true, false,
                    false, true, false,
                    true, true, true,
                ],
                '.' => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, true, false,
                ],
                ' ' => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                ],
                _ => vec![
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                    false, false, false,
                ],
            }
        };
        
        let char_width = 3u32;
        let char_height = 5u32;
        let char_spacing = 1u32;
        
        let mut x_pos = x_offset;
        for c in label.chars() {
            let pattern = get_pattern(c);
            
            for y in 0..char_height {
                for x in 0..char_width {
                    let idx = (y * char_width + x) as usize;
                    let is_set = idx < pattern.len() && pattern[idx];
                    
                    // Draw scaled pixels
                    for sy in 0..pixel_size {
                        for sx in 0..pixel_size {
                            let px = x_pos + x * pixel_size + sx;
                            let py = y_offset + y * pixel_size + sy;
                            
                            if px < self.dimensions.width && py < self.dimensions.height {
                                let buffer_idx = (py * self.dimensions.width + px) as usize * 2;
                                if buffer_idx + 1 < buffer.len() {
                                    let pixel = if is_set { color_label } else { color_bg };
                                    buffer[buffer_idx] = (pixel & 0xFF) as u8;
                                    buffer[buffer_idx + 1] = (pixel >> 8) as u8;
                                }
                            }
                        }
                    }
                }
            }
            
            x_pos += (char_width + char_spacing) * pixel_size;
        }
    }

    pub fn draw_detailed_status(
        &self, 
        qmdl_name: &str, 
        qmdl_size_bytes: usize,
        analysis_size_bytes: usize,
        num_warnings: usize,
        last_warning: Option<&str>,
        color: Color565,
        config: &Config,
        last_msg_time: Option<&str>,
    ) {
        let mut buffer = vec![0; (self.dimensions.width * self.dimensions.height * 2) as usize];
        
        // Set background color based on warnings
        let background_color = if num_warnings > 0 {
            Color565::Red
        } else {
            Color565::Green
        };
        
        // Create initial background
        self.fill_frame(&mut buffer, background_color);
        
        // Choose text color based on background for better contrast
        let text_color = if num_warnings > 0 {
            Color565::White // White text on red background
        } else {
            Color565::Black // Black text on green background
        };
        
        // Use a smaller pixel size to fit everything
        let title_pixel_size = 2;  // Larger title
        let data_pixel_size = 1;   // Smaller data
        let line_height = 12;      // Reduced line height
        
        // Top section spacing - move up slightly
        let header_y = 3;
        let content_x = 10;
        
        // Create a 3D effect with BLACK text and GREY shadows
        // Layer 3 - Deepest shadow (darkest grey, offset by 3 pixels)
        self.draw_enhanced_text(&mut buffer, "RAYHUNTER", 
                               content_x + 3, header_y + 3, 
                               title_pixel_size, 
                               Color565::Blue, // Dark grey shadow
                               Some(background_color));
        
        // Layer 2 - Middle shadow (medium grey, offset by 2 pixels)
        self.draw_enhanced_text(&mut buffer, "RAYHUNTER", 
                               content_x + 2, header_y + 2, 
                               title_pixel_size, 
                               Color565::Cyan, // Medium grey shadow
                               Some(background_color));
                               
        // Layer 1 - Light shadow (light grey, offset by 1 pixel)
        self.draw_enhanced_text(&mut buffer, "RAYHUNTER", 
                               content_x + 1, header_y + 1, 
                               title_pixel_size, 
                               Color565::White, // Light grey shadow
                               Some(background_color));
        
        // Main text (black, no offset)
        self.draw_enhanced_text(&mut buffer, "RAYHUNTER", 
                               content_x, header_y, 
                               title_pixel_size, 
                               Color565::Black, // Black text for main layer
                               Some(background_color));
        
        // Draw version number in smaller text on the right side
        let version_text = format!("v{}", VERSION);
        let version_x = self.dimensions.width - (version_text.len() as u32 * 5 * data_pixel_size) - 5; // Adjust position
        let version_y = header_y + title_pixel_size * 6; // Position just below the header
        
        self.draw_enhanced_text(&mut buffer, &version_text,
                              version_x, version_y,
                              data_pixel_size,
                              text_color,
                              Some(background_color));
        
        // Calculate size in KB (rounded up)
        let size_kb = (qmdl_size_bytes + 1023) / 1024;
        
        // Extract and format dates
        let start_date = if qmdl_name.contains("GMT") || qmdl_name.matches(" ").count() >= 4 { 
            // Format like "Tue Mar 18 2025 03:18:12 GMT-0400"
            let parts: Vec<&str> = qmdl_name.split_whitespace().collect();
            if parts.len() >= 5 {
                let month = parts[1].to_uppercase();
                let day = parts[2];
                let time_parts: Vec<&str> = parts[4].split(':').collect();
                let time = if time_parts.len() >= 2 {
                    format!("{}:{}", time_parts[0], time_parts[1])
                } else {
                    parts[4].to_string()
                };
                
                format!("{} {} {}", month, day, time)
            } else {
                "MAR 18 01:00".to_string()
            }
        } else {
            "MAR 18 01:00".to_string()
        };
        
        // Process last message time
        let last_msg = if let Some(last_msg_str) = last_msg_time {
            if last_msg_str.contains("GMT") || last_msg_str.matches(" ").count() >= 4 {
                let parts: Vec<&str> = last_msg_str.split_whitespace().collect();
                if parts.len() >= 5 {
                    let month = parts[1].to_uppercase();
                    let day = parts[2];
                    let time_parts: Vec<&str> = parts[4].split(':').collect();
                    let time = if time_parts.len() >= 2 {
                        format!("{}:{}", time_parts[0], time_parts[1])
                    } else {
                        parts[4].to_string()
                    };
                    
                    format!("{} {} {}", month, day, time)
                } else {
                    "None".to_string()
                }
            } else {
                last_msg_str.to_string()
            }
        } else {
            "None".to_string()
        };
        
        // Layout with each value below its title - move content up more
        let mut y_pos = header_y + 22; // Reduced from 25 to move content up further
        
        // SIZE section
        self.draw_enhanced_text(&mut buffer, "SIZE:", content_x, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height - 5;
        self.draw_enhanced_text(&mut buffer, &format!("{}KB", size_kb), 
                               content_x + 5, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height + 3; // Reduced spacing
        
        // START section
        self.draw_enhanced_text(&mut buffer, "START:", content_x, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height - 5;
        self.draw_enhanced_text(&mut buffer, &start_date, 
                               content_x + 5, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height + 3; // Reduced spacing
        
        // LAST MSG section
        self.draw_enhanced_text(&mut buffer, "LAST MSG:", content_x, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height - 5;
        self.draw_enhanced_text(&mut buffer, &last_msg, 
                               content_x + 5, y_pos, 
                               data_pixel_size, text_color, 
                               Some(background_color));
        y_pos += line_height + 3; // Add spacing for warnings section
        
        // WARNINGS section - add warnings directly to the main content instead of status bar
        let warnings_text = if num_warnings > 0 {
            format!("WARNINGS: {}", num_warnings)
        } else {
            "WARNINGS: 0".to_string()
        };
        
        // Emphasize warnings with a different style if there are warnings
        if num_warnings > 0 {
            // Draw warning label with emphasis (larger text)
            self.draw_enhanced_text(&mut buffer, &warnings_text, 
                                  content_x, y_pos, 
                                  data_pixel_size + 1, // Make it larger for emphasis
                                  Color565::Yellow, // Yellow for warnings
                                  Some(background_color));
            
            // Add warning icon if there are warnings
            let icon_x = content_x + (warnings_text.len() as u32 * 6 * (data_pixel_size + 1)) + 5;
            let icon_y = y_pos - 2;
            
            // Draw simple exclamation mark as warning icon
            for i in 0..3 {
                for j in 0..7 {
                    if j < 5 || j == 6 {
                        self.draw_pixel(&mut buffer, icon_x + i, icon_y + j, Color565::Yellow);
                    }
                }
            }
        } else {
            // Normal text for no warnings
            self.draw_enhanced_text(&mut buffer, &warnings_text, 
                                  content_x, y_pos, 
                                  data_pixel_size, 
                                  text_color,
                                  Some(background_color));
        }
        
        // Small update animation indicator in bottom right corner
        // Using the static ANIMATION_COUNTER for a simple animation
        unsafe {
            // Use animation counter modulo 4 to create a simple 4-frame animation
            let animation_frame = ANIMATION_COUNTER % 4;
            ANIMATION_COUNTER = ANIMATION_COUNTER.wrapping_add(1);
            
            // Draw animation indicator (small spinning line) in bottom right
            let anim_size = 8;
            let anim_x = self.dimensions.width - anim_size - 5;
            let anim_y = self.dimensions.height - anim_size - 5;
            
            // Clear animation area
            self.draw_rect(&mut buffer, anim_x, anim_y, anim_size, anim_size, background_color);
            
            // Draw animation frame
            match animation_frame {
                0 => {
                    for i in 0..anim_size {
                        self.draw_pixel(&mut buffer, anim_x + i, anim_y + anim_size/2, text_color);
                    }
                },
                1 => {
                    for i in 0..anim_size {
                        self.draw_pixel(&mut buffer, anim_x + anim_size/2, anim_y + i, text_color);
                    }
                },
                2 => {
                    for i in 0..anim_size {
                        self.draw_pixel(&mut buffer, anim_x + anim_size-1-i, anim_y + anim_size/2, text_color);
                    }
                },
                3 => {
                    for i in 0..anim_size {
                        self.draw_pixel(&mut buffer, anim_x + anim_size/2, anim_y + anim_size-1-i, text_color);
                    }
                },
                _ => {}
            }
        }
        
        // Write to framebuffer device
        let _ = fs::write(self.path, &buffer[..]);
    }
    
    // Draw text with enhanced clarity and support for backgrounds
    fn draw_enhanced_text(
        &self,
        buffer: &mut Vec<u8>,
        text: &str,
        x_offset: u32,
        y_offset: u32,
        pixel_size: u32,
        color: Color565,
        background: Option<Color565>,
    ) {
        let mut x = x_offset;
        for c in text.chars() {
            if let Some(pattern) = get_character_pattern(c) {
                self.draw_character(buffer, pattern, x, y_offset, pixel_size, color, background);
                x += 6 * pixel_size; // 5px width + 1px spacing, scaled by pixel size
            }
        }
    }
    
    // Helper to draw a single character
    fn draw_character(
        &self,
        buffer: &mut Vec<u8>,
        pattern: &[u8],
        x_offset: u32,
        y_offset: u32,
        pixel_size: u32,
        color: Color565,
        background: Option<Color565>,
    ) {
        if pattern.len() != 5 * 5 { // Each character is 5x5 pixels
            return;
        }
        
        for py in 0..5 {
            for px in 0..5 {
                let idx = py * 5 + px;
                let is_set = idx < pattern.len() && pattern[idx] == 1;
                
                // Draw filled pixel with the specified size
                for dy in 0..pixel_size {
                    for dx in 0..pixel_size {
                        let draw_x = x_offset + (px as u32 * pixel_size) + dx;
                        let draw_y = y_offset + (py as u32 * pixel_size) + dy;
                        self.draw_pixel(buffer, draw_x, draw_y, if is_set { color } else { background.unwrap_or(Color565::Black) });
                    }
                }
            }
        }
    }

    // Draw a single pixel directly to the buffer
    fn draw_pixel(&self, buffer: &mut Vec<u8>, x: u32, y: u32, color: Color565) {
        if x < self.dimensions.width && y < self.dimensions.height {
            let pixel_index = ((y * self.dimensions.width) + x) as usize * 2;
            if pixel_index + 1 < buffer.len() {
                let color_val = color as u16;
                buffer[pixel_index] = (color_val & 0xFF) as u8;
                buffer[pixel_index + 1] = ((color_val >> 8) & 0xFF) as u8;
            }
        }
    }
    
    // Fill the entire buffer with a single color
    fn fill_frame(&self, buffer: &mut Vec<u8>, color: Color565) {
        let color_val = color as u16;
        for i in 0..buffer.len() / 2 {
            buffer[i * 2] = (color_val & 0xFF) as u8;
            buffer[i * 2 + 1] = ((color_val >> 8) & 0xFF) as u8;
        }
    }
    
    // Draw a filled rectangle
    fn draw_rect(&self, buffer: &mut Vec<u8>, x: u32, y: u32, width: u32, height: u32, color: Color565) {
        for cy in y..y + height {
            if cy >= self.dimensions.height {
                break;
            }
            for cx in x..x + width {
                if cx >= self.dimensions.width {
                    break;
                }
                self.draw_pixel(buffer, cx, cy, color);
            }
        }
    }
}

// Helper function to get character patterns
fn get_character_pattern(c: char) -> Option<&'static [u8]> {
    match c {
        // Letters
        'A' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
        ]),
        'B' => Some(&[
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
        ]),
        'C' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        'D' => Some(&[
            1, 1, 1, 0, 0,
            1, 0, 0, 1, 0,
            1, 0, 0, 0, 1,
            1, 0, 0, 1, 0,
            1, 1, 1, 0, 0,
        ]),
        'E' => Some(&[
            1, 1, 1, 1, 1,
            1, 0, 0, 0, 0,
            1, 1, 1, 0, 0,
            1, 0, 0, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        'F' => Some(&[
            1, 1, 1, 1, 1,
            1, 0, 0, 0, 0,
            1, 1, 1, 0, 0,
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 0,
        ]),
        'G' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 0,
            1, 0, 1, 1, 1,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        'H' => Some(&[
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
        ]),
        'I' => Some(&[
            1, 1, 1, 1, 1,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        'J' => Some(&[
            0, 0, 0, 0, 1,
            0, 0, 0, 0, 1,
            0, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        'K' => Some(&[
            1, 0, 0, 0, 1,
            1, 0, 0, 1, 0,
            1, 1, 1, 0, 0,
            1, 0, 0, 1, 0,
            1, 0, 0, 0, 1,
        ]),
        'L' => Some(&[
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        'M' => Some(&[
            1, 0, 0, 0, 1,
            1, 1, 0, 1, 1,
            1, 0, 1, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
        ]),
        'N' => Some(&[
            1, 0, 0, 0, 1,
            1, 1, 0, 0, 1,
            1, 0, 1, 0, 1,
            1, 0, 0, 1, 1,
            1, 0, 0, 0, 1,
        ]),
        'O' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        'P' => Some(&[
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 0,
            1, 0, 0, 0, 0,
        ]),
        'Q' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 0, 1, 0, 1,
            1, 0, 0, 1, 0,
            0, 1, 1, 0, 1,
        ]),
        'R' => Some(&[
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
            1, 0, 0, 1, 0,
            1, 0, 0, 0, 1,
        ]),
        'S' => Some(&[
            0, 1, 1, 1, 1,
            1, 0, 0, 0, 0,
            0, 1, 1, 1, 0,
            0, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
        ]),
        'T' => Some(&[
            1, 1, 1, 1, 1,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
        ]),
        'U' => Some(&[
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        'V' => Some(&[
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            0, 1, 0, 1, 0,
            0, 0, 1, 0, 0,
        ]),
        'W' => Some(&[
            1, 0, 0, 0, 1,
            1, 0, 0, 0, 1,
            1, 0, 1, 0, 1,
            1, 1, 0, 1, 1,
            1, 0, 0, 0, 1,
        ]),
        'X' => Some(&[
            1, 0, 0, 0, 1,
            0, 1, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 1, 0,
            1, 0, 0, 0, 1,
        ]),
        'Y' => Some(&[
            1, 0, 0, 0, 1,
            0, 1, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
        ]),
        'Z' => Some(&[
            1, 1, 1, 1, 1,
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        // Numbers
        '0' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 1, 1,
            1, 0, 1, 0, 1,
            1, 1, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        '1' => Some(&[
            0, 0, 1, 0, 0,
            0, 1, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 1, 1, 1, 0,
        ]),
        '2' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 0, 1, 1, 0,
            0, 1, 0, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        '3' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 0, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        '4' => Some(&[
            0, 0, 1, 1, 0,
            0, 1, 0, 1, 0,
            1, 0, 0, 1, 0,
            1, 1, 1, 1, 1,
            0, 0, 0, 1, 0,
        ]),
        '5' => Some(&[
            1, 1, 1, 1, 1,
            1, 0, 0, 0, 0,
            1, 1, 1, 1, 0,
            0, 0, 0, 0, 1,
            1, 1, 1, 1, 0,
        ]),
        '6' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 0,
            1, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        '7' => Some(&[
            1, 1, 1, 1, 1,
            0, 0, 0, 0, 1,
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
        ]),
        '8' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        '9' => Some(&[
            0, 1, 1, 1, 0,
            1, 0, 0, 0, 1,
            0, 1, 1, 1, 1,
            0, 0, 0, 0, 1,
            0, 1, 1, 1, 0,
        ]),
        // Special characters
        ' ' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ]),
        '.' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 1, 0, 0, 0,
        ]),
        ':' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 0, 0,
        ]),
        '[' => Some(&[
            0, 1, 1, 0, 0,
            0, 1, 0, 0, 0,
            0, 1, 0, 0, 0,
            0, 1, 0, 0, 0,
            0, 1, 1, 0, 0,
        ]),
        ']' => Some(&[
            0, 0, 1, 1, 0,
            0, 0, 0, 1, 0,
            0, 0, 0, 1, 0,
            0, 0, 0, 1, 0,
            0, 0, 1, 1, 0,
        ]),
        '(' => Some(&[
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
            0, 1, 0, 0, 0,
            0, 1, 0, 0, 0,
            0, 0, 1, 0, 0,
        ]),
        ')' => Some(&[
            0, 0, 1, 0, 0,
            0, 0, 0, 1, 0,
            0, 0, 0, 1, 0,
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
        ]),
        '-' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            1, 1, 1, 1, 1,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ]),
        '_' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            1, 1, 1, 1, 1,
        ]),
        '+' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 1, 1, 1, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 0, 0,
        ]),
        '!' => Some(&[
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 1, 0, 0,
        ]),
        '<' => Some(&[
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 1, 0,
        ]),
        '>' => Some(&[
            0, 1, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
        ]),
        '/' => Some(&[
            0, 0, 0, 0, 1,
            0, 0, 0, 1, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
            1, 0, 0, 0, 0,
        ]),
        '\\' => Some(&[
            1, 0, 0, 0, 0,
            0, 1, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 1, 0,
            0, 0, 0, 0, 1,
        ]),
        '\'' => Some(&[
            0, 0, 1, 0, 0,
            0, 0, 1, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ]),
        ',' => Some(&[
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
            0, 0, 1, 0, 0,
            0, 1, 0, 0, 0,
        ]),
        _ => None,
    }
}