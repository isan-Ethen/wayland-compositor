use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;

const WL_DISPLAY_SYNC: u16 = 0;
const WL_DISPLAY_GET_REGISTRY: u16 = 1;
const WL_REGISTRY_BIND: u16 = 0;
const WL_CALLBACK_DONE: u16 = 0;

const DISPLAY_ID: u32 = 1;
const REGISTRY_ID: u32 = 2;

const WL_COMPOSITOR_NAME: &str = "wl_compositor";
const XDG_WM_BASE_NAME: &str = "xdg_wm_base";
const WL_SHM_NAME: &str = "wl_shm";

struct Client {
    stream: fs::File,
    objects: HashMap<u32, String>,
    next_id: u32,
}

impl Client {
    fn new(stream: fs::File) -> Self {
        let mut objects = HashMap::new();
        objects.insert(DISPLAY_ID, "wl_display".to_string());

        Self {
            stream,
            objects,
            next_id: REGISTRY_ID,
        }
    }

    fn handle_message(&mut self) -> Result<bool, std::io::Error> {
        let mut header = [0u8; 8];
        if let Err(e) = self.stream.read_exact(&mut header) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(false); // Client disconnected
            }
            return Err(e);
        }

        let obj_id = u32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        let size_opcode = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
        let size = size_opcode >> 16;
        let opcode = (size_opcode & 0xFFFF) as u16;

        // Read message body
        let body_size = size as usize - 8;
        let mut body = vec![0u8; body_size];
        self.stream.read_exact(&mut body)?;

        match (obj_id, opcode) {
            (DISPLAY_ID, WL_DISPLAY_SYNC) => {
                let callback_id = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]);
                self.objects.insert(callback_id, "wl_callback".to_string());

                let mut response = vec![
                    callback_id.to_ne_bytes()[0],
                    callback_id.to_ne_bytes()[1],
                    callback_id.to_ne_bytes()[2],
                    callback_id.to_ne_bytes()[3],
                    12,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                ];
                self.stream.write_all(&response)?;
            }
            (DISPLAY_ID, WL_DISPLAY_GET_REGISTRY) => {
                let registry_id = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]);
                self.objects.insert(registry_id, "wl_registry".to_string());
                self.next_id = registry_id + 1;

                // Send registry global events
                self.send_global_event(registry_id, 1, WL_COMPOSITOR_NAME, 4)?;
                self.send_global_event(registry_id, 2, XDG_WM_BASE_NAME, 3)?;
                self.send_global_event(registry_id, 3, WL_SHM_NAME, 1)?;
            }
            (id, WL_REGISTRY_BIND) if self.objects.get(&id) == Some(&"wl_registry".to_string()) => {
            }
            _ => {
                eprintln!("Unknown message: object_id={}, opcode={}", obj_id, opcode);
            }
        }

        Ok(true)
    }

    fn send_global_event(
        &mut self,
        registry_id: u32,
        name: u32,
        interface: &str,
        version: u32,
    ) -> Result<(), std::io::Error> {
        let interface_bytes = interface.as_bytes();
        let interface_len = interface_bytes.len() + 1;
        let aligned_len = (interface_len + 3) & !3;

        let size = 16 + aligned_len;

        let mut msg = Vec::with_capacity(size);

        msg.extend_from_slice(&registry_id.to_ne_bytes());

        msg.extend_from_slice(&((size << 16) as u32).to_ne_bytes());

        msg.extend_from_slice(&name.to_ne_bytes());

        msg.extend_from_slice(interface_bytes);
        msg.push(0);

        while msg.len() < size - 4 {
            msg.push(0);
        }

        msg.extend_from_slice(&version.to_ne_bytes());

        self.stream.write_all(&msg)?;
        Ok(())
    }
}

fn from_syscall_error(error: syscall::Error) -> io::Error {
    io::Error::from_raw_os_error(error.errno as i32)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let xdg_runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let dir = "/tmp/redox-wayland-99".to_string();
        fs::create_dir_all(&dir).expect("Failed to create XDG_RUNTIME_DIR");
        unsafe { env::set_var("XDG_RUNTIME_DIR", &dir) };
        dir
    });

    let socket_name = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let socket_path = Path::new(&xdg_runtime_dir).join(&socket_name);

    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    let scheme_path = format!("chan:{}", socket_path.to_string_lossy());

    let chan_fd = syscall::open(&scheme_path, syscall::O_CREAT | syscall::O_RDWR)
        .map_err(from_syscall_error)?;

    let listener = unsafe { File::from_raw_fd(chan_fd as RawFd) };

    unsafe { env::set_var("WAYLAND_DISPLAY", &socket_name) };

    println!(
        "Redox Minimal Wayland compositor listening on {:?}",
        socket_path
    );
    println!("You can now run 'wayland-info' to connect to this compositor");

    loop {
        let client_fd =
            syscall::dup(listener.as_raw_fd() as usize, b"listen").map_err(from_syscall_error)?;

        let client_stream = unsafe { File::from_raw_fd(client_fd as RawFd) };
        println!("Client connected");

        let mut client = Client::new(client_stream);

        loop {
            match client.handle_message() {
                Ok(true) => continue,
                Ok(false) => {
                    println!("Client disconnected");
                    break;
                }
                Err(e) => {
                    println!("Error handling client message: {}", e);
                    break;
                }
            }
        }
    }
}
