use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use pulse_domain::PROCESS_ICON_SIZE;
use windows::{
    Win32::{
        Graphics::Gdi::*,
        Storage::FileSystem::GetDriveTypeW,
        UI::{
            Shell::ExtractIconExW,
            WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON},
        },
    },
    core::HSTRING,
};

const SIZE: i32 = PROCESS_ICON_SIZE as i32;
const BYTES: usize = PROCESS_ICON_SIZE * PROCESS_ICON_SIZE * 4;
const CAPACITY: usize = 384;
type Pixels = Arc<[u8]>;

#[derive(Default)]
pub struct Icons {
    entries: HashMap<Arc<str>, (Option<Pixels>, Instant)>,
    remaining: usize,
    start: Option<Instant>,
}

impl Icons {
    pub fn begin(&mut self) {
        self.remaining = 8;
        self.start = Some(Instant::now());
    }

    pub fn get(&mut self, path: &Arc<str>) -> Option<Pixels> {
        let now = Instant::now();
        if let Some((image, last)) = self.entries.get_mut(path)
            && (image.is_some() || now.duration_since(*last) < Duration::from_secs(120))
        {
            if image.is_some() {
                *last = now;
            }
            return image.clone();
        }
        if self.remaining == 0
            || self
                .start
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(20))
        {
            return None;
        }
        self.remaining -= 1;
        if self.entries.len() >= CAPACITY
            && !self.entries.contains_key(path)
            && let Some((old, _)) = self.entries.iter().min_by_key(|(_, (_, last))| *last)
        {
            let old = old.clone();
            self.entries.remove(&old);
        }
        let image = extract(path);
        self.entries.insert(path.clone(), (image.clone(), now));
        image
    }
}

fn local_root(path: &str) -> Option<&str> {
    let bytes = path.as_bytes();
    (bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
        && !bytes.contains(&0)
        && path
            .rsplit('\\')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("exe")))
    .then(|| &path[..3])
}

struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}
struct Dc(HDC);
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

fn extract(path: &str) -> Option<Pixels> {
    let root = local_root(path)?;
    // SAFETY: strings are UTF-16 and NUL-terminated. Only fixed drives are accessed.
    if unsafe { GetDriveTypeW(&HSTRING::from(root)) } != 3 {
        return None;
    }
    let mut handle = HICON::default();
    unsafe {
        ExtractIconExW(&HSTRING::from(path), 0, Some(&mut handle), None, 1);
    }
    if handle.is_invalid() {
        return None;
    }
    let icon = Icon(handle);
    // SAFETY: created objects are thread-local. Each allocation is released by its owner.
    let dc = Dc(unsafe { CreateCompatibleDC(None) });
    if dc.0.is_invalid() {
        return None;
    }
    let mut bits = std::ptr::null_mut();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            biHeight: -SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let bitmap = Bitmap(unsafe {
        CreateDIBSection(Some(dc.0), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?
    });
    if bits.is_null() {
        return None;
    }
    let previous = unsafe { SelectObject(dc.0, HGDIOBJ(bitmap.0.0)) };
    if previous.is_invalid() {
        return None;
    }
    // Drawing over two backgrounds recovers alpha without relying on unused DIB alpha bytes.
    let draw = |background: u8| -> Option<[u8; BYTES]> {
        // SAFETY: the top-down 32bpp DIB exposes exactly BYTES, and writes are flushed
        // before CPU reads and before the next draw modifies the same memory.
        unsafe {
            std::ptr::write_bytes(bits.cast::<u8>(), background, BYTES);
            let result = DrawIconEx(dc.0, 0, 0, icon.0, SIZE, SIZE, 0, None, DI_NORMAL);
            let flushed = GdiFlush().as_bool();
            if result.is_err() || !flushed {
                return None;
            }
            Some(std::ptr::read(bits.cast::<[u8; BYTES]>()))
        }
    };
    let black = draw(0);
    let white = draw(255);
    unsafe {
        SelectObject(dc.0, previous);
    }
    let rgba = reconstruct(&black?, &white?);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] != 0)
        .then(|| Arc::from(rgba))
}

fn reconstruct(black: &[u8; BYTES], white: &[u8; BYTES]) -> Vec<u8> {
    let mut rgba = vec![0; BYTES];
    for ((b, w), out) in black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(white.as_chunks::<4>().0)
        .zip(rgba.as_chunks_mut::<4>().0)
    {
        let transmission = (0..3)
            .map(|i| u32::from(w[i].saturating_sub(b[i])))
            .sum::<u32>();
        let alpha = 255 - (transmission + 1) / 3;
        if alpha == 0 {
            continue;
        }
        for i in 0..3 {
            out[i] = ((u32::from(b[2 - i]) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
        out[3] = alpha as u8;
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_paths_exclude_unc_and_non_executables() {
        assert_eq!(local_root(r"C:\Windows\example.EXE"), Some(r"C:\"));
        assert_eq!(local_root(r"\\server\share\example.exe"), None);
        assert_eq!(local_root(r"C:\Windows\example.dll"), None);
    }
    #[test]
    fn reconstructs_alpha_and_swaps_bgr() {
        let b = [0, 0, 128, 0].repeat(BYTES / 4).try_into().unwrap();
        let w = [127, 127, 255, 255].repeat(BYTES / 4).try_into().unwrap();
        assert_eq!(reconstruct(&b, &w)[..4], [255, 0, 0, 128]);
    }

    #[test]
    fn extracts_a_real_windows_executable_icon() {
        let mut directory = [0; 256];
        // SAFETY: Windows writes at most the provided UTF-16 buffer length.
        let len = unsafe {
            windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut directory))
        } as usize;
        assert!(len > 0 && len < directory.len());
        let path = format!(r"{}\cmd.exe", String::from_utf16_lossy(&directory[..len]));
        let icon = extract(&path).expect("Windows command processor has an icon");
        assert_eq!(icon.len(), BYTES);
        assert!(icon.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
    }
}
