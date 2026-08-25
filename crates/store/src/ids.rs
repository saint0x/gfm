use gfm_types::{FileId, GfmError, Result, VolumeId};
use std::io::{Cursor, Read, Write};
use std::path::Path;

const DEFAULT_ID_BLOCK_SIZE: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdBlock {
    first: FileId,
    entries: u64,
    len: u64,
}

pub(crate) fn write_blocked_file_ids(
    mut writer: impl Write,
    ids: &[FileId],
) -> std::io::Result<()> {
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
    let block_size = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    if block_size == 0 {
        return Err(id_format_error(path, "zero id block size"));
    }
    let blocks = read_id_blocks(&mut reader, path)?;
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
        ids.extend(read_delta_file_ids(
            Cursor::new(bytes),
            block.entries,
            path,
        )?);
        offset = end;
    }
    if ids.len() != count as usize {
        return Err(id_format_error(path, "id count mismatch"));
    }
    Ok(ids)
}

pub(crate) fn read_blocked_file_id_block_from_slice(
    bytes: &[u8],
    block_index: usize,
    path: &Path,
) -> Result<Vec<FileId>> {
    let mut reader = Cursor::new(bytes);
    let _count = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    let block_size = read_varint(&mut reader).map_err(|err| GfmError::io(path, err))?;
    if block_size == 0 {
        return Err(id_format_error(path, "zero id block size"));
    }
    let blocks = read_id_blocks(&mut reader, path)?;
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
    read_delta_file_ids(Cursor::new(bytes), target.entries, path)
}

fn block_payload_len(blocks: &[IdBlock], path: &Path) -> Result<usize> {
    blocks.iter().try_fold(0usize, |sum, block| {
        let len = usize::try_from(block.len)
            .map_err(|_| id_format_error(path, "id block length overflow"))?;
        sum.checked_add(len)
            .ok_or_else(|| id_format_error(path, "id block range overflow"))
    })
}

fn read_id_blocks(mut reader: impl Read, path: &Path) -> Result<Vec<IdBlock>> {
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
            len,
        });
    }
    Ok(blocks)
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

fn read_delta_file_ids(mut reader: impl Read, count: u64, path: &Path) -> Result<Vec<FileId>> {
    let mut ids = Vec::with_capacity(count.min(1_000_000) as usize);
    let mut previous = FileId::new(VolumeId(0), 0);
    for _ in 0..count {
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
    Ok(ids)
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

        write_blocked_file_ids(&mut encoded, &ids).unwrap();

        let decoded = read_blocked_file_ids(Cursor::new(&encoded), path).unwrap();
        let block = read_blocked_file_id_block_from_slice(&encoded, 1, path).unwrap();

        assert_eq!(decoded, ids);
        assert_eq!(block.len(), DEFAULT_ID_BLOCK_SIZE);
        assert_eq!(block[0], FileId::new(VolumeId(7), 10_128));
    }
}
