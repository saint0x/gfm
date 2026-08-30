use gfm_types::{FileId, GfmError, Result, VolumeId};
use std::io::{Cursor, Read, Write};
use std::path::Path;

const DEFAULT_ID_BLOCK_SIZE: usize = 128;
const BLOCKED_IDS_V2_MARKER: u64 = 0;
const BLOCKED_IDS_V2_VERSION: u64 = 2;
const ID_DECODE_CHECK_STRIDE: u64 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdBlock {
    first: FileId,
    entries: u64,
    codec: IdBlockCodec,
    len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdBlockCodec {
    Delta,
    Run,
}

pub(crate) fn write_blocked_file_ids(writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    write_blocked_file_ids_v2(writer, ids)
}

fn write_blocked_file_ids_v2(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    write_varint(&mut writer, ids.len() as u64)?;
    write_varint(&mut writer, BLOCKED_IDS_V2_MARKER)?;
    write_varint(&mut writer, BLOCKED_IDS_V2_VERSION)?;
    write_varint(&mut writer, DEFAULT_ID_BLOCK_SIZE as u64)?;

    let mut encoded = Vec::new();
    let mut blocks = Vec::new();
    for block_ids in ids.chunks(DEFAULT_ID_BLOCK_SIZE) {
        let first = block_ids[0];
        let (codec, block) = encode_best_block(block_ids)?;
        blocks.push(IdBlock {
            first,
            entries: block_ids.len() as u64,
            codec,
            len: block.len() as u64,
        });
        encoded.extend(block);
    }

    write_varint(&mut writer, blocks.len() as u64)?;
    for block in &blocks {
        write_varint(&mut writer, block.first.volume.0)?;
        write_varint(&mut writer, block.first.node)?;
        write_varint(&mut writer, block.entries)?;
        write_varint(&mut writer, block.codec.as_code())?;
        write_varint(&mut writer, block.len)?;
    }
    writer.write_all(&encoded)
}

#[cfg(test)]
fn write_blocked_file_ids_v1(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    write_varint(&mut writer, ids.len() as u64)?;
    write_varint(&mut writer, DEFAULT_ID_BLOCK_SIZE as u64)?;

    let mut encoded = Vec::new();
    let mut blocks = Vec::new();
    for block_ids in ids.chunks(DEFAULT_ID_BLOCK_SIZE) {
        let first = block_ids[0];
        let mut block = Vec::new();
        write_delta_file_ids(&mut block, block_ids)?;
        blocks.push(IdBlock {
            first,
            entries: block_ids.len() as u64,
            codec: IdBlockCodec::Delta,
            len: block.len() as u64,
        });
        encoded.extend(block);
    }

    write_varint(&mut writer, blocks.len() as u64)?;
    for block in &blocks {
        write_varint(&mut writer, block.first.volume.0)?;
        write_varint(&mut writer, block.first.node)?;
        write_varint(&mut writer, block.entries)?;
        write_varint(&mut writer, block.len)?;
    }
    writer.write_all(&encoded)
}

pub(crate) fn read_blocked_file_ids(mut reader: impl Read, path: &Path) -> Result<Vec<FileId>> {
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let (_block_size, blocks) = read_blocked_header(&mut reader, path)?;
    let mut payload = vec![0; block_payload_len(&blocks, path)?];
    reader
        .read_exact(&mut payload)
        .map_err(|err| GfmError::io(path, err))?;

    let mut offset = 0usize;
    let mut ids = Vec::with_capacity(count.min(1_000_000) as usize);
    for block in blocks {
        let len = usize::try_from(block.len)
            .map_err(|_| id_format_error(path, "id block length overflow"))?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
        let bytes = payload
            .get(offset..end)
            .ok_or_else(|| id_format_error(path, "id block out of bounds"))?;
        ids.extend(read_encoded_file_ids(Cursor::new(bytes), &block, path)?);
        offset = end;
    }
    if ids.len() != count as usize {
        return Err(id_format_error(path, "id count mismatch"));
    }
    Ok(ids)
}

pub(crate) fn read_blocked_file_ids_limited_from_slice_checked(
    bytes: &[u8],
    limit: usize,
    path: &Path,
    check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<FileId>> {
    read_blocked_file_ids_limited_report_from_slice_checked(bytes, limit, path, check_control)
        .map(|report| report.ids)
}

pub(crate) fn read_blocked_file_ids_for_volume_limited_from_slice_checked(
    bytes: &[u8],
    volume: VolumeId,
    limit: usize,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<(Vec<FileId>, bool)> {
    check_control()?;
    let mut reader = Cursor::new(bytes);
    let _count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let (_block_size, blocks) = read_blocked_header(&mut reader, path)?;
    check_control()?;
    let payload_start = usize::try_from(reader.position())
        .map_err(|_| id_format_error(path, "id block payload offset overflow"))?;
    let payload_len = block_payload_len(&blocks, path)?;
    let encoded_len = payload_start
        .checked_add(payload_len)
        .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    if bytes.get(..encoded_len).is_none() {
        return Err(id_format_error(path, "id block out of bounds"));
    }
    if limit == 0 {
        return Ok((Vec::new(), false));
    }

    let mut payload_offset = 0usize;
    let mut ids = Vec::with_capacity(limit.min(DEFAULT_ID_BLOCK_SIZE));
    for (index, block) in blocks.iter().enumerate() {
        check_control()?;
        let len = usize::try_from(block.len)
            .map_err(|_| id_format_error(path, "id block length overflow"))?;
        let start = payload_start
            .checked_add(payload_offset)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;

        let next_first = blocks.get(index + 1).map(|next| next.first);
        if block.first.volume > volume {
            break;
        }
        if next_first.is_some_and(|next| next.volume < volume) {
            payload_offset = payload_offset
                .checked_add(len)
                .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
            continue;
        }

        let block_bytes = bytes
            .get(start..end)
            .ok_or_else(|| id_format_error(path, "id block out of bounds"))?;
        for id in read_encoded_file_ids_checked(
            Cursor::new(block_bytes),
            block,
            path,
            &mut check_control,
        )? {
            check_control()?;
            if id.volume < volume {
                continue;
            }
            if id.volume > volume {
                return Ok((ids, false));
            }
            ids.push(id);
            if ids.len() > limit {
                ids.truncate(limit);
                return Ok((ids, true));
            }
        }
        payload_offset = payload_offset
            .checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    }
    Ok((ids, false))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockedFileIdsLimitedReport {
    pub ids: Vec<FileId>,
    pub truncated: bool,
    pub encoded_len: usize,
}

pub(crate) fn read_blocked_file_ids_limited_report_from_slice_checked(
    bytes: &[u8],
    limit: usize,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<BlockedFileIdsLimitedReport> {
    check_control()?;
    let mut reader = Cursor::new(bytes);
    let count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    check_control()?;
    let (_block_size, blocks) = read_blocked_header(&mut reader, path)?;
    check_control()?;
    let payload_start = usize::try_from(reader.position())
        .map_err(|_| id_format_error(path, "id block payload offset overflow"))?;
    let payload_len = block_payload_len(&blocks, path)?;
    let encoded_len = payload_start
        .checked_add(payload_len)
        .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    if bytes.get(..encoded_len).is_none() {
        return Err(id_format_error(path, "id block out of bounds"));
    }
    if limit == 0 {
        return Ok(BlockedFileIdsLimitedReport {
            ids: Vec::new(),
            truncated: count > 0,
            encoded_len,
        });
    }
    let mut payload_offset = 0usize;
    let mut ids = Vec::with_capacity(limit.min(DEFAULT_ID_BLOCK_SIZE));
    for block in &blocks {
        check_control()?;
        let len = usize::try_from(block.len)
            .map_err(|_| id_format_error(path, "id block length overflow"))?;
        let start = payload_start
            .checked_add(payload_offset)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
        let block_bytes = bytes
            .get(start..end)
            .ok_or_else(|| id_format_error(path, "id block out of bounds"))?;
        ids.extend(read_encoded_file_ids_checked(
            Cursor::new(block_bytes),
            block,
            path,
            &mut check_control,
        )?);
        if ids.len() >= limit {
            ids.truncate(limit);
            return Ok(BlockedFileIdsLimitedReport {
                ids,
                truncated: count as usize > limit,
                encoded_len,
            });
        }
        payload_offset = payload_offset
            .checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    }
    Ok(BlockedFileIdsLimitedReport {
        ids,
        truncated: false,
        encoded_len,
    })
}

pub(crate) fn read_blocked_file_id_block_from_slice(
    bytes: &[u8],
    block_index: usize,
    path: &Path,
) -> Result<Vec<FileId>> {
    let mut reader = Cursor::new(bytes);
    let _count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let (_block_size, blocks) = read_blocked_header(&mut reader, path)?;
    let target = blocks
        .get(block_index)
        .ok_or_else(|| id_format_error(path, "id block index out of bounds"))?;
    let skip_len = block_payload_len(&blocks[..block_index], path)?;
    let target_len = usize::try_from(target.len)
        .map_err(|_| id_format_error(path, "id block length overflow"))?;
    let payload_start = usize::try_from(reader.position())
        .map_err(|_| id_format_error(path, "id block payload offset overflow"))?;
    let start = payload_start
        .checked_add(skip_len)
        .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    let end = start
        .checked_add(target_len)
        .ok_or_else(|| id_format_error(path, "id block range overflow"))?;
    let bytes = bytes
        .get(start..end)
        .ok_or_else(|| id_format_error(path, "id block out of bounds"))?;
    read_encoded_file_ids(Cursor::new(bytes), target, path)
}

fn read_blocked_header(mut reader: impl Read, path: &Path) -> Result<(u64, Vec<IdBlock>)> {
    let block_size = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    if block_size == BLOCKED_IDS_V2_MARKER {
        let version = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        if version != BLOCKED_IDS_V2_VERSION {
            return Err(id_format_error(path, "unsupported blocked id version"));
        }
        let block_size = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        if block_size == 0 {
            return Err(id_format_error(path, "zero id block size"));
        }
        Ok((block_size, read_id_blocks_v2(reader, path)?))
    } else {
        Ok((block_size, read_id_blocks_v1(reader, path)?))
    }
}

fn block_payload_len(blocks: &[IdBlock], path: &Path) -> Result<usize> {
    blocks.iter().try_fold(0usize, |sum, block| {
        let len = usize::try_from(block.len)
            .map_err(|_| id_format_error(path, "id block length overflow"))?;
        sum.checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))
    })
}

fn read_id_blocks_v1(mut reader: impl Read, path: &Path) -> Result<Vec<IdBlock>> {
    let block_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut blocks = Vec::with_capacity(block_count.min(1_000_000) as usize);
    for _ in 0..block_count {
        let volume = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let entries = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        blocks.push(IdBlock {
            first: FileId::new(VolumeId(volume), node),
            entries,
            codec: IdBlockCodec::Delta,
            len,
        });
    }
    Ok(blocks)
}

fn read_id_blocks_v2(mut reader: impl Read, path: &Path) -> Result<Vec<IdBlock>> {
    let block_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut blocks = Vec::with_capacity(block_count.min(1_000_000) as usize);
    for _ in 0..block_count {
        let volume = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let entries = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let codec = IdBlockCodec::from_code(
            read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?,
            path,
        )?;
        let len = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        blocks.push(IdBlock {
            first: FileId::new(VolumeId(volume), node),
            entries,
            codec,
            len,
        });
    }
    Ok(blocks)
}

impl IdBlockCodec {
    const fn as_code(self) -> u64 {
        match self {
            Self::Delta => 0,
            Self::Run => 1,
        }
    }

    fn from_code(value: u64, path: &Path) -> Result<Self> {
        match value {
            0 => Ok(Self::Delta),
            1 => Ok(Self::Run),
            _ => Err(id_format_error(path, "unsupported id block codec")),
        }
    }
}

fn encode_best_block(ids: &[FileId]) -> std::io::Result<(IdBlockCodec, Vec<u8>)> {
    let mut delta = Vec::new();
    write_delta_file_ids(&mut delta, ids)?;
    let mut run = Vec::new();
    write_run_file_ids(&mut run, ids)?;
    if run.len() < delta.len() {
        Ok((IdBlockCodec::Run, run))
    } else {
        Ok((IdBlockCodec::Delta, delta))
    }
}

fn read_encoded_file_ids(reader: impl Read, block: &IdBlock, path: &Path) -> Result<Vec<FileId>> {
    read_encoded_file_ids_checked(reader, block, path, || Ok(()))
}

fn read_encoded_file_ids_checked(
    reader: impl Read,
    block: &IdBlock,
    path: &Path,
    check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<FileId>> {
    match block.codec {
        IdBlockCodec::Delta => {
            read_delta_file_ids_checked(reader, block.entries, path, check_control)
        }
        IdBlockCodec::Run => read_run_file_ids_checked(reader, block.entries, path, check_control),
    }
}

fn write_delta_file_ids(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let mut previous = FileId::new(VolumeId(0), 0);
    for id in ids {
        write_varint(&mut writer, id.volume.0.saturating_sub(previous.volume.0))?;
        let node_delta = if id.volume == previous.volume {
            id.node.saturating_sub(previous.node)
        } else {
            id.node
        };
        write_varint(&mut writer, node_delta)?;
        previous = *id;
    }
    Ok(())
}

fn write_run_file_ids(mut writer: impl Write, ids: &[FileId]) -> std::io::Result<()> {
    let runs = id_runs(ids);
    write_varint(&mut writer, runs.len() as u64)?;
    let mut previous = FileId::new(VolumeId(0), 0);
    for run in runs {
        write_varint(
            &mut writer,
            run.start.volume.0.saturating_sub(previous.volume.0),
        )?;
        let node_delta = if run.start.volume == previous.volume {
            run.start.node.saturating_sub(previous.node)
        } else {
            run.start.node
        };
        write_varint(&mut writer, node_delta)?;
        write_varint(&mut writer, run.entries)?;
        previous = FileId::new(
            run.start.volume,
            run.start.node.saturating_add(run.entries.saturating_sub(1)),
        );
    }
    Ok(())
}

fn read_delta_file_ids_checked(
    mut reader: impl Read,
    count: u64,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<FileId>> {
    check_control()?;
    let mut ids = Vec::with_capacity(count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for index in 0..count {
        if index.is_multiple_of(ID_DECODE_CHECK_STRIDE) {
            check_control()?;
        }
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| id_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| id_format_error(path, "file node id overflow"))?
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

fn read_run_file_ids_checked(
    mut reader: impl Read,
    count: u64,
    path: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<FileId>> {
    check_control()?;
    let run_count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let mut ids = Vec::with_capacity(count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    let mut decoded = 0u64;
    for _ in 0..run_count {
        check_control()?;
        let volume_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let volume = previous
            .volume
            .0
            .checked_add(volume_delta)
            .ok_or_else(|| id_format_error(path, "volume id overflow"))?;
        let node_delta = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        let start_node = if volume == previous.volume.0 {
            previous
                .node
                .checked_add(node_delta)
                .ok_or_else(|| id_format_error(path, "file node id overflow"))?
        } else {
            node_delta
        };
        let entries = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
        if entries == 0 {
            return Err(id_format_error(path, "empty id run"));
        }
        for offset in 0..entries {
            if decoded.is_multiple_of(ID_DECODE_CHECK_STRIDE) {
                check_control()?;
            }
            let node = start_node
                .checked_add(offset)
                .ok_or_else(|| id_format_error(path, "file node id overflow"))?;
            ids.push(FileId::new(VolumeId(volume), node));
            decoded += 1;
        }
        previous = FileId::new(
            VolumeId(volume),
            start_node
                .checked_add(entries - 1)
                .ok_or_else(|| id_format_error(path, "file node id overflow"))?,
        );
    }
    if ids.len() != count as usize {
        return Err(id_format_error(path, "id run count mismatch"));
    }
    check_control()?;
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IdRun {
    start: FileId,
    entries: u64,
}

fn id_runs(ids: &[FileId]) -> Vec<IdRun> {
    let Some((&first, rest)) = ids.split_first() else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    let mut start = first;
    let mut previous = first;
    let mut entries = 1u64;
    for id in rest {
        if id.volume == previous.volume && id.node == previous.node.saturating_add(1) {
            entries += 1;
        } else {
            runs.push(IdRun { start, entries });
            start = *id;
            entries = 1;
        }
        previous = *id;
    }
    runs.push(IdRun { start, entries });
    runs
}

fn write_varint(mut writer: impl Write, mut value: u64) -> std::io::Result<()> {
    while value >= 0x80 {
        writer.write_all(&[((value as u8) & 0x7f) | 0x80])?;
        value >>= 7;
    }
    writer.write_all(&[value as u8])
}

fn read_varint(mut reader: impl Read) -> std::io::Result<u64> {
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

fn id_format_error(path: &Path, reason: &str) -> GfmError {
    GfmError::Format(format!(
        "invalid id block store {}: {reason}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_file_ids_round_trip_and_decode_single_block() {
        let path = Path::new("/tmp/gfm-id-block-test");
        let ids = (0..300)
            .map(|node| FileId::new(VolumeId(7), 10_000 + node))
            .chain([FileId::new(VolumeId(9), 1), FileId::new(VolumeId(9), 2)])
            .collect::<Vec<_>>();
        let mut encoded = Vec::new();
        let mut legacy = Vec::new();

        write_blocked_file_ids(&mut encoded, &ids).unwrap();
        write_blocked_file_ids_v1(&mut legacy, &ids).unwrap();

        let decoded = read_blocked_file_ids(Cursor::new(&encoded), path).unwrap();
        let legacy_decoded = read_blocked_file_ids(Cursor::new(&legacy), path).unwrap();
        let block = read_blocked_file_id_block_from_slice(&encoded, 1, path).unwrap();
        let legacy_block = read_blocked_file_id_block_from_slice(&legacy, 1, path).unwrap();

        assert_eq!(decoded, ids);
        assert_eq!(legacy_decoded, ids);
        assert!(encoded.len() < legacy.len());
        assert_eq!(block.len(), DEFAULT_ID_BLOCK_SIZE);
        assert_eq!(block[0], FileId::new(VolumeId(7), 10_128));
        assert_eq!(legacy_block, block);
    }

    #[test]
    fn blocked_file_ids_keep_sparse_blocks_delta_encoded() {
        let path = Path::new("/tmp/gfm-id-block-sparse-test");
        let ids = (0..200)
            .map(|index| FileId::new(VolumeId(11), 10_000 + index * 257))
            .collect::<Vec<_>>();
        let mut encoded = Vec::new();
        let mut legacy = Vec::new();

        write_blocked_file_ids(&mut encoded, &ids).unwrap();
        write_blocked_file_ids_v1(&mut legacy, &ids).unwrap();

        assert_eq!(
            read_blocked_file_ids(Cursor::new(&encoded), path).unwrap(),
            ids
        );
        assert_eq!(
            read_blocked_file_id_block_from_slice(&encoded, 1, path).unwrap()[0],
            FileId::new(VolumeId(11), 10_000 + 128 * 257)
        );
        assert!(encoded.len() <= legacy.len() + 8);
    }

    #[test]
    fn checked_limited_blocked_file_ids_cancel_during_run_decode() {
        let path = Path::new("/tmp/gfm-id-block-run-cancel-test");
        let ids = (0..1024)
            .map(|node| FileId::new(VolumeId(7), 10_000 + node))
            .collect::<Vec<_>>();
        let mut encoded = Vec::new();
        let mut checks = 0usize;

        write_blocked_file_ids(&mut encoded, &ids).unwrap();
        let result = read_blocked_file_ids_limited_from_slice_checked(&encoded, 1024, path, || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(checks, 5);
    }

    #[test]
    fn checked_volume_limited_blocked_file_ids_cancel_during_run_decode() {
        let path = Path::new("/tmp/gfm-id-block-volume-run-cancel-test");
        let ids = (0..1024)
            .map(|node| FileId::new(VolumeId(7), 10_000 + node))
            .collect::<Vec<_>>();
        let mut encoded = Vec::new();
        let mut checks = 0usize;

        write_blocked_file_ids(&mut encoded, &ids).unwrap();
        let result = read_blocked_file_ids_for_volume_limited_from_slice_checked(
            &encoded,
            VolumeId(7),
            1024,
            path,
            || {
                checks += 1;
                if checks >= 5 {
                    Err(GfmError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(GfmError::Cancelled)));
        assert_eq!(checks, 5);
    }
}
