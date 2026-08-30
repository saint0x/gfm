use super::{content_format_error, ContentStoreVersion};
use crate::ids::{
    read_blocked_file_id_block_from_slice, read_blocked_file_ids_checked,
    read_blocked_file_ids_for_volume_limited_from_slice_checked,
    read_blocked_file_ids_limited_report_from_slice_checked, write_blocked_file_ids,
};
use gfm_types::{ContentPositions, ContentPosting, FileId, GfmError, Result, VolumeId};
use std::io::{Cursor, Read, Write};
use std::path::Path;

const CONTENT_DECODE_CHECK_STRIDE: u64 = 256;

pub(super) fn write_content_posting(
    mut writer: impl Write,
    posting: &ContentPosting,
    version: ContentStoreVersion,
) -> std::io::Result<()> {
    let term = posting.term.as_bytes();
    write_varint(&mut writer, term.len() as u64)?;
    writer.write_all(term)?;
    if version.uses_blocked_ids() {
        write_blocked_file_ids(&mut writer, &posting.ids)?;
    } else {
        write_file_ids(&mut writer, &posting.ids)?;
    }
    if version.uses_positions() {
        write_content_positions(writer, &posting.positions)
    } else {
        Ok(())
    }
}

pub(super) fn read_content_posting(
    reader: impl Read,
    path: &Path,
    version: ContentStoreVersion,
) -> Result<ContentPosting> {
    read_content_posting_checked(reader, path, version, || Ok(()))
}

pub(super) fn read_content_posting_checked(
    mut reader: impl Read,
    path: &Path,
    version: ContentStoreVersion,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<ContentPosting> {
    check_control()?;
    let term = read_content_posting_term(&mut reader, path)?;
    check_control()?;
    let ids = if version.uses_blocked_ids() {
        read_blocked_file_ids_checked(&mut reader, path, &mut check_control)?
    } else {
        read_file_ids_checked(&mut reader, path, &mut check_control)?
    };
    check_control()?;
    let positions = if version.uses_positions() {
        read_content_positions_checked(reader, path, &mut check_control)?
    } else {
        Vec::new()
    };
    check_control()?;
    Ok(ContentPosting {
        term,
        ids,
        positions,
    })
}

pub(super) fn read_content_posting_limited_from_slice_checked(
    bytes: &[u8],
    path: &Path,
    version: ContentStoreVersion,
    limit: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ContentPosting, bool)> {
    check_control()?;
    let mut cursor = Cursor::new(bytes);
    let term = read_content_posting_term(&mut cursor, path)?;
    check_control()?;
    let ids_start = usize::try_from(cursor.position())
        .map_err(|_| content_format_error(path, "content id offset overflow"))?;
    let id_bytes = bytes
        .get(ids_start..)
        .ok_or_else(|| content_format_error(path, "content id offset out of bounds"))?;
    check_control()?;

    let (ids, ids_truncated, positions_start) = if version.uses_blocked_ids() {
        let report = read_blocked_file_ids_limited_report_from_slice_checked(
            id_bytes,
            limit,
            path,
            &mut check_control,
        )?;
        let positions_start = ids_start
            .checked_add(report.encoded_len)
            .ok_or_else(|| content_format_error(path, "content position offset overflow"))?;
        (report.ids, report.truncated, positions_start)
    } else {
        let mut id_cursor = Cursor::new(id_bytes);
        let ids = read_file_ids_checked(&mut id_cursor, path, &mut check_control)?;
        let truncated = ids.len() > limit;
        let ids = ids.into_iter().take(limit).collect();
        let consumed = usize::try_from(id_cursor.position())
            .map_err(|_| content_format_error(path, "content id offset overflow"))?;
        let positions_start = ids_start
            .checked_add(consumed)
            .ok_or_else(|| content_format_error(path, "content position offset overflow"))?;
        (ids, truncated, positions_start)
    };

    let (positions, positions_truncated) = if version.uses_positions() {
        let position_bytes = bytes
            .get(positions_start..)
            .ok_or_else(|| content_format_error(path, "content position range out of bounds"))?;
        read_content_positions_limited_checked(
            Cursor::new(position_bytes),
            path,
            limit,
            &mut check_control,
        )?
    } else {
        (Vec::new(), false)
    };
    check_control()?;

    Ok((
        ContentPosting {
            term,
            ids,
            positions,
        },
        ids_truncated || positions_truncated,
    ))
}

pub(super) fn read_content_posting_for_volume_limited_from_slice_checked(
    bytes: &[u8],
    path: &Path,
    version: ContentStoreVersion,
    volume: VolumeId,
    limit: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(ContentPosting, bool)> {
    check_control()?;
    let mut cursor = Cursor::new(bytes);
    let term = read_content_posting_term(&mut cursor, path)?;
    check_control()?;
    let ids_start = usize::try_from(cursor.position())
        .map_err(|_| content_format_error(path, "content id offset overflow"))?;
    let id_bytes = bytes
        .get(ids_start..)
        .ok_or_else(|| content_format_error(path, "content id offset out of bounds"))?;
    check_control()?;

    let (ids, ids_truncated, positions_start) = if version.uses_blocked_ids() {
        let report = read_blocked_file_ids_limited_report_from_slice_checked(
            id_bytes,
            0,
            path,
            &mut check_control,
        )?;
        let (ids, truncated) = read_blocked_file_ids_for_volume_limited_from_slice_checked(
            id_bytes,
            volume,
            limit,
            path,
            &mut check_control,
        )?;
        let positions_start = ids_start
            .checked_add(report.encoded_len)
            .ok_or_else(|| content_format_error(path, "content position offset overflow"))?;
        (ids, truncated, positions_start)
    } else {
        let mut id_cursor = Cursor::new(id_bytes);
        let ids = read_file_ids_checked(&mut id_cursor, path, &mut check_control)?;
        let matching_count = ids.iter().filter(|id| id.volume == volume).count();
        let ids = ids
            .into_iter()
            .filter(|id| id.volume == volume)
            .take(limit)
            .collect();
        let consumed = usize::try_from(id_cursor.position())
            .map_err(|_| content_format_error(path, "content id offset overflow"))?;
        let positions_start = ids_start
            .checked_add(consumed)
            .ok_or_else(|| content_format_error(path, "content position offset overflow"))?;
        (ids, matching_count > limit, positions_start)
    };

    let (positions, positions_truncated) = if version.uses_positions() {
        let position_bytes = bytes
            .get(positions_start..)
            .ok_or_else(|| content_format_error(path, "content position range out of bounds"))?;
        read_content_positions_for_volume_limited_checked(
            Cursor::new(position_bytes),
            path,
            volume,
            limit,
            &mut check_control,
        )?
    } else {
        (Vec::new(), false)
    };
    check_control()?;

    Ok((
        ContentPosting {
            term,
            ids,
            positions,
        },
        ids_truncated || positions_truncated,
    ))
}

pub(super) fn read_content_posting_term(mut reader: impl Read, path: &Path) -> Result<String> {
    let term_len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut term = vec![0; term_len as usize];
    reader
        .read_exact(&mut term)
        .map_err(|err| GfmError::io(path, err))?;
    let term = String::from_utf8(term).map_err(|err| {
        GfmError::Format(format!("invalid UTF-8 term in {}: {err}", path.display()))
    })?;
    Ok(term)
}

fn write_content_positions(
    mut writer: impl Write,
    positions: &[ContentPositions],
) -> std::io::Result<()> {
    let mut positions = positions.to_vec();
    positions.sort_by(|left, right| left.id.cmp(&right.id));
    write_varint(&mut writer, positions.len() as u64)?;
    let mut previous = FileId::new(VolumeId(0), 0);
    for entry in positions {
        write_varint(
            &mut writer,
            entry.id.volume.0.saturating_sub(previous.volume.0),
        )?;
        let node_delta = if entry.id.volume == previous.volume {
            entry.id.node.saturating_sub(previous.node)
        } else {
            entry.id.node
        };
        write_varint(&mut writer, node_delta)?;
        let mut offsets = entry.positions;
        offsets.sort_unstable();
        offsets.dedup();
        write_varint(&mut writer, offsets.len() as u64)?;
        let mut previous_position = 0u32;
        for position in offsets {
            write_varint(
                &mut writer,
                position.saturating_sub(previous_position) as u64,
            )?;
            previous_position = position;
        }
        previous = entry.id;
    }
    Ok(())
}

fn read_content_positions_checked(
    mut reader: impl Read,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ContentPositions>> {
    check_control()?;
    let entry_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut entries = Vec::with_capacity(entry_count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for entry_index in 0..entry_count {
        if entry_index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
            check_control()?;
        }
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| content_format_error(path, "position volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "position file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        let position_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let mut positions = Vec::with_capacity(position_count.min(1_000_000) as usize);
        let mut previous_position = 0u32;
        for position_index in 0..position_count {
            if position_index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
                check_control()?;
            }
            let delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
            let delta = u32::try_from(delta)
                .map_err(|_| content_format_error(path, "content position overflow"))?;
            let position = previous_position
                .checked_add(delta)
                .ok_or_else(|| content_format_error(path, "content position overflow"))?;
            positions.push(position);
            previous_position = position;
        }
        entries.push(ContentPositions { id, positions });
        previous = id;
    }
    check_control()?;
    Ok(entries)
}

fn read_content_positions_limited_checked(
    mut reader: impl Read,
    path: &Path,
    limit: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(Vec<ContentPositions>, bool)> {
    check_control()?;
    let entry_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let entry_count_usize = usize::try_from(entry_count)
        .map_err(|_| content_format_error(path, "content position count overflow"))?;
    let read_entries = entry_count_usize.min(limit);
    let mut entries = Vec::with_capacity(read_entries);
    let mut previous = FileId::new(VolumeId(0), 0);
    for entry_index in 0..read_entries {
        if (entry_index as u64).is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
            check_control()?;
        }
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| content_format_error(path, "position volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "position file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        let position_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let mut positions = Vec::with_capacity(position_count.min(1_000_000) as usize);
        let mut previous_position = 0u32;
        for position_index in 0..position_count {
            if position_index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
                check_control()?;
            }
            let delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
            let delta = u32::try_from(delta)
                .map_err(|_| content_format_error(path, "content position overflow"))?;
            let position = previous_position
                .checked_add(delta)
                .ok_or_else(|| content_format_error(path, "content position overflow"))?;
            positions.push(position);
            previous_position = position;
        }
        entries.push(ContentPositions { id, positions });
        previous = id;
    }
    check_control()?;
    Ok((entries, entry_count_usize > limit))
}

fn read_content_positions_for_volume_limited_checked(
    mut reader: impl Read,
    path: &Path,
    volume: VolumeId,
    limit: usize,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(Vec<ContentPositions>, bool)> {
    check_control()?;
    let entry_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut entries = Vec::with_capacity(limit.min(128));
    let mut previous = FileId::new(VolumeId(0), 0);
    for entry_index in 0..entry_count {
        if entry_index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
            check_control()?;
        }
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let entry_volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| content_format_error(path, "position volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if entry_volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "position file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(entry_volume), node);
        let position_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        if id.volume > volume {
            return Ok((entries, false));
        }
        if id.volume == volume && entries.len() >= limit {
            return Ok((entries, true));
        }
        let mut positions = Vec::new();
        let read_positions = id.volume == volume && entries.len() < limit;
        if read_positions {
            positions = Vec::with_capacity(position_count.min(1_000_000) as usize);
        }
        let mut previous_position = 0u32;
        for position_index in 0..position_count {
            if position_index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
                check_control()?;
            }
            let delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
            let delta = u32::try_from(delta)
                .map_err(|_| content_format_error(path, "content position overflow"))?;
            let position = previous_position
                .checked_add(delta)
                .ok_or_else(|| content_format_error(path, "content position overflow"))?;
            if read_positions {
                positions.push(position);
            }
            previous_position = position;
        }
        if read_positions {
            entries.push(ContentPositions { id, positions });
        }
        previous = id;
    }
    check_control()?;
    Ok((entries, false))
}

pub(super) fn write_file_ids(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    write_varint(&mut writer, ids.len() as u64)?;
    let mut previous = FileId::new(VolumeId(0), 0);
    for id in ids {
        write_varint(&mut writer, id.volume.0.saturating_sub(previous.volume.0))?;
        let node_delta = if id.volume == previous.volume {
            id.node.saturating_sub(previous.node)
        } else {
            id.node
        };
        write_varint(&mut writer, node_delta)?;
        previous = id;
    }
    Ok(())
}

pub(crate) fn read_file_ids(mut reader: impl Read, path: &Path) -> Result<Vec<FileId>> {
    read_file_ids_checked(&mut reader, path, || Ok(()))
}

pub(crate) fn read_file_ids_checked(
    mut reader: impl Read,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<FileId>> {
    check_control()?;
    let id_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut ids = Vec::with_capacity(id_count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for index in 0..id_count {
        if index.is_multiple_of(CONTENT_DECODE_CHECK_STRIDE) {
            check_control()?;
        }
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| content_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| content_format_error(path, "file node id overflow"))?
        } else {
            node_delta
        };
        let id = FileId::new(VolumeId(volume), node);
        ids.push(id);
        previous = id;
    }
    check_control()?;
    Ok(ids)
}

pub(super) fn read_blocked_id_block(
    bytes: &[u8],
    block_index: usize,
    path: &Path,
) -> Result<Vec<FileId>> {
    read_blocked_file_id_block_from_slice(bytes, block_index, path)
}

pub(super) fn write_varint(mut writer: impl Write, mut value: u64) -> std::io::Result<()> {
    while value >= 0x80 {
        writer.write_all(&[((value as u8) & 0x7f) | 0x80])?;
        value >>= 7;
    }
    writer.write_all(&[value as u8])
}

pub(crate) fn read_varint(mut reader: impl Read) -> std::io::Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)?;
        value |= ((byte[0] & 0x7f) as u64) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "varint overflow",
            ));
        }
    }
}
