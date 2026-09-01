//! Files-first range scheduler, then split the largest remaining file (AD-12).

/// Inclusive-exclusive byte ranges covering `file_len` in `range_len` chunks.
pub fn plan_ranges(file_len: u64, range_len: u64) -> Vec<(u64, u64)> {
    if file_len == 0 || range_len == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0_u64;
    while offset < file_len {
        let len = range_len.min(file_len - offset);
        out.push((offset, len));
        offset += len;
    }
    out
}

pub fn remaining(planned: &[(u64, u64)], sidecar: &super::sidecar::Sidecar) -> Vec<(u64, u64)> {
    planned
        .iter()
        .copied()
        .filter(|(offset, len)| !sidecar.has(*offset, *len))
        .collect()
}

#[derive(Debug, Clone)]
pub struct PendingFile {
    pub index: usize,
    pub remaining: Vec<(u64, u64)>,
}

/// Fill `n` slots: one range per file first, then split the largest remaining.
pub fn take_slots(files: &mut [PendingFile], n: usize) -> Vec<(usize, u64, u64)> {
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    for file in files.iter_mut() {
        if out.len() >= n {
            break;
        }
        if !file.remaining.is_empty() {
            let (offset, len) = file.remaining.remove(0);
            out.push((file.index, offset, len));
        }
    }
    while out.len() < n {
        let Some(pos) = files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.remaining.is_empty())
            .max_by_key(|(_, f)| f.remaining.iter().map(|r| r.1).sum::<u64>())
            .map(|(i, _)| i)
        else {
            break;
        };
        let (offset, len) = files[pos].remaining.remove(0);
        out.push((files[pos].index, offset, len));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_ranges_covers_the_file() {
        assert_eq!(plan_ranges(10, 4), vec![(0, 4), (4, 4), (8, 2)]);
        assert!(plan_ranges(0, 4).is_empty());
        assert!(plan_ranges(10, 0).is_empty());
    }

    #[test]
    fn files_first_then_split_largest() {
        let mut files = vec![
            PendingFile {
                index: 0,
                remaining: vec![(0, 8), (8, 8), (16, 8)],
            },
            PendingFile {
                index: 1,
                remaining: vec![(0, 4)],
            },
        ];
        let slots = take_slots(&mut files, 4);
        assert_eq!(slots, vec![(0, 0, 8), (1, 0, 4), (0, 8, 8), (0, 16, 8)]);
    }
}
