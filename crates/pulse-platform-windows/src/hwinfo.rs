//! Optional, read-only consumer of HWiNFO's Shared Memory Interface (SM2).
use super::{OwnedHandle, error};
use pulse_domain::{Availability, CpuThermal};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, WAIT_ABANDONED, WAIT_OBJECT_0},
        System::{
            Memory::{
                FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
                UnmapViewOfFile,
            },
            Threading::{
                OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
            },
        },
    },
    core::{HRESULT, w},
};

const HEADER: usize = 44;
const LIMIT: usize = 8 * 1024 * 1024;

struct View(MEMORY_MAPPED_VIEW_ADDRESS);
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: this guard owns the mapped view.
        unsafe {
            let _ = UnmapViewOfFile(self.0);
        }
    }
}
impl View {
    fn open(mapping: &OwnedHandle, len: usize) -> Option<Self> {
        // SAFETY: a valid mapping is held; Windows validates the requested extent.
        let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, len) };
        if view.Value.is_null() {
            None
        } else {
            Some(Self(view))
        }
    }
    fn copy(&self, bytes: &mut [u8]) {
        // SAFETY: the destination is independent and sized to the mapped view by the caller.
        // The publisher's mutex is held for the copy.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.0.Value.cast::<u8>(),
                bytes.as_mut_ptr(),
                bytes.len(),
            );
        }
    }
}

struct Lock(OwnedHandle);
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: drop runs on the owning thread, after a successful wait.
        unsafe {
            let _ = ReleaseMutex(self.0.0);
        }
    }
}

pub fn sample(now_ms: u64) -> Availability<CpuThermal> {
    use Availability::*;
    // SAFETY: the names are static, and handles are never inherited or created by PULSE.
    let mutex = match unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            w!(r"Global\HWiNFO_SM2_MUTEX"),
        )
    } {
        Ok(handle) => OwnedHandle(handle),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => return Unsupported,
        Err(e) => return error(e),
    };
    let mapping = match unsafe {
        OpenFileMappingW(FILE_MAP_READ.0, false, w!(r"Global\HWiNFO_SENS_SM2"))
    } {
        Ok(handle) => OwnedHandle(handle),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => return Unsupported,
        Err(e) => return error(e),
    };
    // SAFETY: valid mutex handle. A zero wait never stalls monitoring.
    let wait = unsafe { WaitForSingleObject(mutex.0, 0) };
    if wait == WAIT_ABANDONED {
        drop(Lock(mutex));
        return Pending;
    }
    if wait != WAIT_OBJECT_0 {
        return Pending;
    }
    let lock = Lock(mutex);
    let Some(header) = View::open(&mapping, HEADER) else {
        return Pending;
    };
    let mut bytes = [0; HEADER];
    header.copy(&mut bytes);
    let Some(layout) = Layout::parse(&bytes, now_ms) else {
        return Pending;
    };
    let size = layout.sensors.end.max(layout.readings.end);
    let Some(view) = View::open(&mapping, size) else {
        return Pending;
    };
    let mut bytes = vec![0; size];
    view.copy(&mut bytes);
    drop(lock);
    layout.temperature(&bytes).map_or(Unsupported, Available)
}

#[derive(Debug)]
struct Section {
    start: usize,
    stride: usize,
    end: usize,
}
struct Layout {
    sensors: Section,
    readings: Section,
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}
fn text(bytes: &[u8]) -> &[u8] {
    bytes.split(|&b| b == 0).next().unwrap_or_default()
}

impl Layout {
    fn parse(b: &[u8], now_ms: u64) -> Option<Self> {
        if b.get(..4)? != b"HWiS" || !matches!(u32_at(b, 4)?, 1 | 2) {
            return None;
        }
        let sec = u64_at(b, 12)?;
        let now = now_ms / 1000;
        if sec == 0 || sec > now.saturating_add(2) || now.saturating_sub(sec) > 30 {
            return None;
        }
        let section =
            |i, minimum| {
                let start = u32_at(b, i)? as usize;
                let stride = u32_at(b, i + 4)? as usize;
                let count = u32_at(b, i + 8)? as usize;
                let end = start.checked_add(stride.checked_mul(count)?)?;
                (start >= HEADER && end <= LIMIT && stride >= minimum && count > 0)
                    .then_some(Section { start, stride, end })
            };
        let sensors = section(20, 264)?;
        let readings = section(32, 316)?;
        if sensors.end > readings.start {
            return None;
        }
        Some(Self { sensors, readings })
    }
    fn temperature(&self, b: &[u8]) -> Option<CpuThermal> {
        let sensors = b.get(self.sensors.start..self.sensors.end)?;
        let readings = b.get(self.readings.start..self.readings.end)?;
        readings
            .chunks_exact(self.readings.stride)
            .filter_map(|r| {
                if u32_at(r, 0)? != 1 {
                    return None;
                }
                let offset = (u32_at(r, 4)? as usize).checked_mul(self.sensors.stride)?;
                let group = text(sensors.get(offset + 8..offset + 136)?);
                if !group.starts_with(b"CPU [#") {
                    return None;
                }
                let unit = text(&r[268..284]);
                if !matches!(unit, b"\xb0C" | b"\xc2\xb0C" | b"C") {
                    return None;
                }
                let name = text(&r[12..140]);
                let (priority, label) = if name.eq_ignore_ascii_case(b"CPU (Tctl/Tdie)") {
                    (3, "CPU (Tctl/Tdie)")
                } else if name.eq_ignore_ascii_case(b"CPU Package") {
                    (2, "CPU Package")
                } else if name.eq_ignore_ascii_case(b"CPU Die (average)") {
                    (1, "CPU Die (average)")
                } else {
                    return None;
                };
                let celsius = f64::from_bits(u64_at(r, 284)?);
                (0. ..=150.)
                    .contains(&celsius)
                    .then_some((priority, CpuThermal { celsius, label }))
            })
            .max_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.celsius.total_cmp(&b.1.celsius))
            })
            .map(|(_, r)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_cpu_package_only_and_rejects_stale_or_invalid_metadata() {
        let mut b = vec![0; 44 + 264 + 316];
        b[..4].copy_from_slice(b"HWiS");
        for (offset, val) in [
            (4, 2_u32),
            (12, 99),
            (20, 44),
            (24, 264),
            (28, 1),
            (32, 308),
            (36, 316),
            (40, 1),
            (308, 1),
        ] {
            b[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
        }
        b[52..60].copy_from_slice(b"CPU [#0]");
        let label = b"CPU (Tctl/Tdie)";
        b[320..320 + label.len()].copy_from_slice(label);
        b[576..578].copy_from_slice(b"\xb0C");
        b[592..600].copy_from_slice(&61.25_f64.to_le_bytes());
        let layout = Layout::parse(&b, 100_000).unwrap();
        assert_eq!(layout.temperature(&b).unwrap().celsius, 61.25);
        assert!(Layout::parse(&b, 132_000).is_none());
        b[52..60].copy_from_slice(b"GPU [#0]");
        assert!(layout.temperature(&b).is_none());
        b[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Layout::parse(&b, 100_000).is_none());
    }
}
