use crate::OwnedHandle;
use pulse_domain::DriveSample;
use windows::{
    Win32::{
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{IO::DeviceIoControl, Ioctl::*},
    },
    core::HSTRING,
};

const DATA: usize = 512;
const PROTOCOL: usize = std::mem::size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>();
const PREFIX: usize = std::mem::offset_of!(STORAGE_PROPERTY_QUERY, AdditionalParameters);
const HEADER: usize = std::mem::size_of::<STORAGE_PROTOCOL_DATA_DESCRIPTOR>();

pub fn read(sample: &mut DriveSample) {
    let path = HSTRING::from(format!(r"\\.\PhysicalDrive{}", sample.index));
    // SAFETY: query-only access (zero desired access); the fixed path comes from the Windows inventory.
    let Ok(handle) = (unsafe {
        CreateFileW(
            &path,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }) else {
        return;
    };
    let device = OwnedHandle(handle);
    for property in [
        StorageDeviceProtocolSpecificProperty,
        StorageAdapterProtocolSpecificProperty,
    ] {
        let mut buffer = [0u32; (PREFIX + PROTOCOL + DATA) / 4];
        buffer[..8].copy_from_slice(&[
            property.0 as u32,
            PropertyStandardQuery.0 as u32,
            ProtocolTypeNvme.0 as u32,
            NVMeDataTypeLogPage.0 as u32,
            2, // SMART / Health Information Log (NVMe log page 02h).
            0,
            PROTOCOL as u32,
            DATA as u32,
        ]);
        let ptr = buffer.as_mut_ptr().cast();
        let len = std::mem::size_of_val(&buffer) as u32;
        let mut written = 0;
        // SAFETY: documented IOCTL using one initialized, aligned, fixed-size input/output buffer.
        if unsafe {
            DeviceIoControl(
                device.0,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(ptr),
                len,
                Some(ptr),
                len,
                Some(&mut written),
                None,
            )
        }
        .is_err()
        {
            continue;
        }
        if written > len
            || (written as usize) < HEADER
            || buffer[0] as usize != HEADER
            || buffer[1] as usize != HEADER
            || buffer[2] != ProtocolTypeNvme.0 as u32
            || buffer[3] != NVMeDataTypeLogPage.0 as u32
        {
            continue;
        }
        let offset = buffer[6] as usize;
        let returned = buffer[7] as usize;
        if offset < PROTOCOL || returned < DATA {
            continue;
        }
        // SAFETY: the shared view of initialized u32 storage is limited to bytes returned by the API.
        let raw =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), written as usize) };
        let Some(page) = PREFIX
            .checked_add(offset)
            .and_then(|begin| begin.checked_add(DATA).and_then(|end| raw.get(begin..end)))
        else {
            continue;
        };
        apply(sample, page);
        return;
    }
}

fn apply(sample: &mut DriveSample, page: &[u8]) {
    if page.len() != DATA || page.iter().all(|&byte| byte == 0) {
        return;
    }
    let k = u16::from_le_bytes([page[1], page[2]]);
    sample.nvme_smart = true;
    sample.health = sample.health.filter(|&v| v <= 2).map_or_else(
        || Some(u16::from(page[0] != 0)),
        |v| Some(v.max(u16::from(page[0] != 0))),
    );
    sample.temperature = k
        .checked_sub(273)
        .filter(|v| (1..=150).contains(v))
        .map(|v| v as u8)
        .or(sample.temperature);
    // PercentageUsed may exceed 100 on worn drives; 255 denotes a value >= 255%.
    sample.wear = Some(page[5]);
    sample.power_on_hours = counter(page, 128)
        .and_then(|n| u64::try_from(n).ok())
        .or(sample.power_on_hours);
    sample.written_bytes = counter(page, 48)
        .filter(|&n| n > 0)
        .and_then(|n| n.checked_mul(512_000))
        .and_then(|n| u64::try_from(n).ok());
}

fn counter(page: &[u8], offset: usize) -> Option<u128> {
    Some(u128::from_le_bytes(
        page.get(offset..offset + 16)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_counter_units_and_health_without_overflow() {
        let mut page = [0; DATA];
        page[0] = 1;
        page[1..3].copy_from_slice(&315_u16.to_le_bytes());
        page[5] = 102;
        page[48..64].copy_from_slice(&100_u128.to_le_bytes());
        page[128..144].copy_from_slice(&203_u128.to_le_bytes());
        let mut d = DriveSample::default();
        apply(&mut d, &page);
        assert_eq!(
            (
                d.temperature,
                d.wear,
                d.written_bytes,
                d.power_on_hours,
                d.health
            ),
            (Some(42), Some(102), Some(51_200_000), Some(203), Some(1))
        );
        page[48..64].fill(0xff);
        apply(&mut d, &page);
        assert!(d.written_bytes.is_none());
    }
}
