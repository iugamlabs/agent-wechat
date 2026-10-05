use crate::ia::types::MediaResult;
use crate::tools::wechat_db::{get_db_path, query_wechat_db};
use crate::tools::wechat_messages::{decode_message_content, extract_xml_tag, find_message_db, get_msg_table_name};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, Default, serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ImageQuality {
    Legacy,
    Full,
    Standard,
    Thumbnail,
    #[default]
    Best,
}

fn image_suffixes(quality: ImageQuality) -> &'static [&'static str] {
    match quality {
        ImageQuality::Legacy => &["", "_t", "_h"],
        ImageQuality::Full => &["_h"],
        ImageQuality::Standard => &[""],
        ImageQuality::Thumbnail => &["_t"],
        ImageQuality::Best => &["_h", "", "_t"],
    }
}

fn non_image_quality(quality: ImageQuality) -> ImageQuality {
    match quality {
        ImageQuality::Best => ImageQuality::Legacy,
        ImageQuality::Standard => ImageQuality::Full,
        other => other,
    }
}

/// WeChat .dat file magic bytes: 07 08 56 32 08 07
const DAT_MAGIC: [u8; 6] = [0x07, 0x08, 0x56, 0x32, 0x08, 0x07];

struct ImageKeys {
    aes_key_hex: String,
    xor_byte: Option<u8>,
}

fn unsupported() -> MediaResult {
    MediaResult {
        media_type: "unsupported".into(),
        data: None,
        url: None,
        format: String::new(),
        filename: String::new(),
        quality: None,
    }
}

pub(crate) fn pending() -> MediaResult {
    MediaResult {
        media_type: "pending".into(),
        data: None,
        url: None,
        format: String::new(),
        filename: String::new(),
        quality: None,
    }
}

fn account_base_paths(account_dir: &str) -> [String; 2] {
    [
        format!("/home/wechat/xwechat_files/{account_dir}"),
        format!("/home/wechat/Documents/xwechat_files/{account_dir}"),
    ]
}

fn native_user_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 &&
        value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn native_group_id(value: &str) -> bool {
    value.strip_suffix("@chatroom").map(|id|
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_digit())
    ).unwrap_or(false)
}

/// Group bodies may include a sender prefix that is separate from the native XML.
fn native_message_body(chat: &str, sender: &str, account: &str, content: String) -> Option<String> {
    if !native_user_id(sender) { return None; }
    if native_group_id(chat) {
        if let Some((prefix, body)) = content.split_once(":\n") {
            if !prefix.starts_with('<') {
                return (prefix == sender).then(|| body.to_string());
            }
        }
    } else if sender != chat && sender != account { return None; }
    if chat == "filehelper" && sender != account { return None; }
    Some(content)
}

/// Native requests are hydrated from stored, completed message rows.
/// Voice construction remains disabled until separately validated.
pub fn download_metadata(account: &str, keys: &HashMap<String, String>, chat: &str, id: i64) -> Option<serde_json::Value> {
    if id <= 0 || id > u32::MAX as i64 ||
        (!native_user_id(chat) && !native_group_id(chat)) { return None; }
    let (name, key) = find_message_db(account, keys, chat)?;
    let path = get_db_path(account, &name);
    let table = get_msg_table_name(chat);
    let rows = query_wechat_db(&path, &key, &format!(
        "SELECT m.local_id, m.local_type, CAST(m.server_id AS TEXT) AS server_id,
         CAST(m.sort_seq AS TEXT) AS sort_seq, m.create_time,
         hex(m.message_content) AS body, m.WCDB_CT_message_content AS compressed,
         n.user_name AS sender FROM \"{table}\" m
         LEFT JOIN Name2Id n ON n.rowid=m.real_sender_id WHERE m.local_id={id} LIMIT 1;"
    ));
    let row = rows.first()?;
    let sender = row.get("sender")?.as_str()?;
    let kind = row.get("local_type")?.as_i64()?;
    if kind != 3 && kind != 43 && kind != (6i64 << 32 | 49) { return None; }
    let content = decode_message_content(row.get("body")?.as_str()?, row.get("compressed")?.as_i64()? != 0);
    if content.is_empty() || content.len() > 1024 * 1024 || content.contains('\0') { return None; }
    if kind == (6i64 << 32 | 49) {
        if !safe_attachment_name(&extract_xml_tag(&content, "title")?) { return None; }
        extract_xml_tag(&content, "totallen")?.parse::<u64>().ok()?;
        let hash = extract_xml_tag(&content, "md5")?;
        if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    } else if kind == 43 {
        video_attr(&content, "length")?.parse::<u64>().ok()?;
        let hash = video_attr(&content, "md5")?;
        if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    }
    // Resolve the exact account ID from the same database, not by splitting wxid.
    let escaped = account.replace('\'', "''");
    let names = query_wechat_db(&path, &key, &format!(
        "SELECT user_name FROM Name2Id WHERE user_name='{escaped}' OR
         substr('{escaped}',1,length(user_name)+1)=user_name||'_';"
    ));
    let matches: Vec<&str> = names.iter().filter_map(|r| r.get("user_name")?.as_str())
        .filter(|name| *name == account || account.strip_prefix(*name).map(|tail|
            tail.len() == 5 && tail.starts_with('_') && tail[1..].bytes().all(|b| b.is_ascii_hexdigit())
        ).unwrap_or(false)).collect();
    if matches.len() != 1 || matches[0] == chat { return None; }
    let content = native_message_body(chat, sender, matches[0], content)?;
    Some(serde_json::json!({
        "accountId": matches[0], "chatId": chat, "senderId": sender, "local_id": id, "local_type": kind,
        "server_id": row.get("server_id")?.as_str()?, "sort_seq": row.get("sort_seq")?.as_str()?,
        "create_time": row.get("create_time")?.as_i64()?, "content": content,
    }))
}

/// Look up a single message's raw content by localId.
fn lookup_message_raw(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
) -> Option<(i64, i64, String)> {
    let table_name = get_msg_table_name(chat_id);
    let (db_name, key) = find_message_db(account_dir, keys, chat_id)?;
    let db_path = get_db_path(account_dir, &db_name);

    let rows = query_wechat_db(
        &db_path,
        key,
        &format!(
            "SELECT local_type, create_time,
                    hex(message_content) as hex_content,
                    WCDB_CT_message_content as is_compressed
             FROM \"{table_name}\"
             WHERE local_id = {local_id}
             LIMIT 1;"
        ),
    );

    let row = rows.first()?;
    let local_type = row.get("local_type")?.as_i64()?;
    let create_time = row.get("create_time").and_then(|v| v.as_i64()).unwrap_or(0);
    let hex_content = row
        .get("hex_content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_compressed = row
        .get("is_compressed")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        != 0;

    let content = decode_message_content(hex_content, is_compressed);
    // Strip group sender prefix
    let body = if let Some(idx) = content.find(":\n") {
        if idx < 80 {
            content[idx + 2..].to_string()
        } else {
            content
        }
    } else {
        content
    };

    Some((local_type, create_time, body))
}

/// Extract an XML attribute value.
fn xml_attr(xml: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=\"");
    let start = xml.find(&pat)? + pat.len();
    let end = xml[start..].find('"')? + start;
    let val = xml[start..end].trim().to_string();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

/// Extract an exact video XML attribute without matching `rawlength`/`rawmd5`.
fn video_attr(xml: &str, attr: &str) -> Option<String> {
    let pat = format!(" {attr}=\"");
    let start = xml.find(&pat)? + pat.len();
    let end = xml[start..].find('"')? + start;
    let val = xml[start..end].trim().to_string();
    (!val.is_empty()).then_some(val)
}

/// Highest image variant advertised by the sender. A regular phone send may
/// offer a mid-size image but no original, so waiting for _h.dat cannot help.
pub(crate) fn best_image_target(content: &str) -> &'static str {
    if video_attr(content, "cdnbigimgurl").is_some()
        || video_attr(content, "hdlength").and_then(|value| value.parse::<u64>().ok()).unwrap_or(0) > 0
    {
        "full"
    } else if video_attr(content, "cdnmidimgurl").is_some() {
        "standard"
    } else {
        "thumbnail"
    }
}

// ── Image thumbnail from filesystem cache ────────────────────────────────────

fn get_image_thumbnail(
    account_dir: &str,
    chat_id: &str,
    local_id: i64,
    create_time: i64,
) -> Option<MediaResult> {
    let hash = format!("{:x}", Md5::digest(chat_id.as_bytes()));
    let dt = chrono::DateTime::from_timestamp(create_time, 0)?;
    let year_month = dt.format("%Y-%m").to_string();
    let thumb_name = format!("{local_id}_{create_time}_thumb.jpg");

    for base in &account_base_paths(account_dir) {
        let thumb_path = Path::new(base)
            .join("cache")
            .join(&year_month)
            .join("Message")
            .join(&hash)
            .join("Thumb")
            .join(&thumb_name);
        if thumb_path.exists() {
            if let Ok(data) = fs::read(&thumb_path) {
                if let Some((data, _)) = convert_media("normalize-image", &data) {
                    return Some(MediaResult {
                        media_type: "image".into(),
                        data: Some(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &data,
                        )),
                        url: None,
                        format: "jpeg".into(),
                        filename: format!("msg_{local_id}.jpg"),
                        quality: None,
                    });
                }
            }
        }

        // Fallback: find any thumbnail matching this localId
        let thumb_dir = Path::new(base)
            .join("cache")
            .join(&year_month)
            .join("Message")
            .join(&hash)
            .join("Thumb");
        if let Ok(entries) = fs::read_dir(&thumb_dir) {
            let prefix = format!("{local_id}_");
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) {
                    if let Ok(data) = fs::read(entry.path()) {
                        if let Some((data, _)) = convert_media("normalize-image", &data) {
                            return Some(MediaResult {
                                media_type: "image".into(),
                                data: Some(base64::Engine::encode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &data,
                                )),
                                url: None,
                                format: "jpeg".into(),
                                filename: format!("msg_{local_id}.jpg"),
                                quality: None,
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

// ── .dat file decryption ─────────────────────────────────────────────────────

fn aligned_aes_size(enc_chunk_size: u32) -> Option<usize> {
    let size = enc_chunk_size as usize;
    size.checked_add(16 - size % 16)
}

fn decrypt_dat_head(dat: &[u8], aes_key_hex: &str) -> Option<(Vec<u8>, u32)> {
    if dat.len() < 15 || dat[..6] != DAT_MAGIC {
        return None;
    }
    let enc_chunk_size = u32::from_le_bytes(dat[6..10].try_into().ok()?);
    let aes_key = aes_key_hex.as_bytes().get(..16)?; // first 16 ASCII chars

    let aligned = aligned_aes_size(enc_chunk_size)?;
    if dat.len() < 15 + aligned {
        return None;
    }
    let ct = &dat[15..15 + aligned];

    // AES-128-ECB decrypt via openssl CLI (no native Rust AES dep needed)
    let mut child = Command::new("openssl")
        .args(["enc", "-d", "-aes-128-ecb", "-K"])
        .arg(hex_encode(aes_key))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    use std::io::Write;
    child.stdin.take()?.write_all(ct).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    Some((output.stdout, enc_chunk_size))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn derive_xor_byte(dat: &[u8], dec_head: &[u8]) -> Option<u8> {
    if dat.len() < 2 {
        return None;
    }
    if dec_head.len() >= 2 && dec_head[0] == 0xff && dec_head[1] == 0xd8 {
        // JPEG: last 2 bytes should be FF D9
        let c1 = dat[dat.len() - 2] ^ 0xFF;
        let c2 = dat[dat.len() - 1] ^ 0xD9;
        if c1 == c2 {
            return Some(c1);
        }
    }
    if dec_head.len() >= 4 && dec_head[..4] == [0x89, 0x50, 0x4e, 0x47] {
        // PNG: last 8 bytes are IEND chunk
        let expected = [0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
        if dat.len() >= 8 {
            let ts = dat.len() - 8;
            let xb = dat[ts] ^ expected[0];
            if expected
                .iter()
                .enumerate()
                .all(|(i, &e)| (dat[ts + i] ^ xb) == e)
            {
                return Some(xb);
            }
        }
    }
    if dec_head.len() >= 4 && &dec_head[..4] == b"GIF8" {
        // GIF: last 2 bytes are 00 3B
        let c1 = dat[dat.len() - 2] ^ 0x00;
        let c2 = dat[dat.len() - 1] ^ 0x3B;
        if c1 == c2 {
            return Some(c1);
        }
    }
    None
}

/// Bound JPEG recovery across the requested image and at most two sibling thumbnails.
fn infer_xor_byte(dat: &[u8], image_keys: &ImageKeys, deadline: std::time::Instant) -> Option<u8> {
    let (head, _) = decrypt_dat_head(dat, &image_keys.aes_key_hex)?;
    let xor_size = u32::from_le_bytes(dat[10..14].try_into().ok()?);
    if xor_size == 0 {
        return Some(0);
    }
    if !head.starts_with(&[0xff, 0xd8]) {
        return derive_xor_byte(dat, &head);
    }
    let decrypted = decrypt_dat(dat, &image_keys.aes_key_hex, 0)?;
    let remaining = deadline.checked_duration_since(std::time::Instant::now())?;
    let mut input = Vec::new();
    input.extend_from_slice(&xor_size.to_le_bytes());
    input.extend_from_slice(&(remaining.as_millis().min(3000) as u32).to_le_bytes());
    input.extend_from_slice(&decrypted);
    let (result, _) = convert_media("recover-jpeg-xor", &input)?;
    if result.len() == 1 {
        Some(result[0])
    } else {
        None
    }
}

fn resolve_xor_byte(dat_path: &str, dat: &[u8], image_keys: &ImageKeys) -> Option<u8> {
    if let Some(xb) = image_keys.xor_byte {
        return Some(xb);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    if let Some(xb) = infer_xor_byte(dat, image_keys, deadline) {
        return Some(xb);
    }
    let dir = Path::new(dat_path).parent()?;
    // Prefer this image's own thumbnail before unrelated directory entries.
    let path = Path::new(dat_path);
    let stem = path.file_stem()?.to_str()?;
    let stem = stem
        .strip_suffix("_h")
        .or_else(|| stem.strip_suffix("_t"))
        .unwrap_or(stem);
    let matching_thumb = dir.join(format!("{stem}_t.dat"));
    let mut attempts = 0;
    if matching_thumb != path && std::time::Instant::now() < deadline {
        if let Ok(sibling) = fs::read(&matching_thumb) {
            attempts += 1;
            if let Some(xb) = infer_xor_byte(&sibling, image_keys, deadline) {
                return Some(xb);
            }
        }
    }
    let entries = fs::read_dir(dir).ok()?;
    // Limit directory work and decoder attempts. Never alter WeChat cache files.
    for entry in entries.take(64).flatten() {
        if std::time::Instant::now() >= deadline || attempts >= 2 {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with("_t.dat") || entry.path() == path || entry.path() == matching_thumb {
            continue;
        }
        attempts += 1;
        if let Ok(sibling) = fs::read(entry.path()) {
            if let Some(xb) = infer_xor_byte(&sibling, image_keys, deadline) {
                return Some(xb);
            }
        }
    }
    None
}

fn decrypt_dat(dat: &[u8], aes_key_hex: &str, xor_byte: u8) -> Option<Vec<u8>> {
    let (dec_head, enc_chunk_size) = decrypt_dat_head(dat, aes_key_hex)?;
    let xor_size = u32::from_le_bytes(dat[10..14].try_into().ok()?) as usize;
    let aes_ct_end = 15 + aligned_aes_size(enc_chunk_size)?;
    let remaining = &dat[aes_ct_end..];

    let raw_length = remaining.len().checked_sub(xor_size)?;
    let raw_data = &remaining[..raw_length];
    let xor_data = &remaining[raw_length..];

    let dec_tail: Vec<u8> = xor_data.iter().map(|b| b ^ xor_byte).collect();

    let mut result = Vec::with_capacity(dec_head.len() + raw_data.len() + dec_tail.len());
    result.extend_from_slice(&dec_head);
    result.extend_from_slice(raw_data);
    result.extend_from_slice(&dec_tail);
    Some(result)
}

fn detect_image_format(data: &[u8]) -> (&'static str, &'static str) {
    if data.len() >= 2 && data[0] == 0xff && data[1] == 0xd8 {
        return ("jpeg", "jpg");
    }
    if data.len() >= 4 && data[..4] == [0x89, 0x50, 0x4e, 0x47] {
        return ("png", "png");
    }
    if data.len() >= 4 && &data[..4] == b"GIF8" {
        return ("gif", "gif");
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return ("webp", "webp");
    }
    if data.len() >= 4 && &data[..4] == b"wxgf" {
        return ("wxgf", "wxgf");
    }
    ("unknown", "bin")
}

/// Convert media via the media-convert tool.
fn convert_media(mode: &str, input: &[u8]) -> Option<(Vec<u8>, String)> {
    use std::io::Write;
    #[cfg(not(test))]
    let mut converter = Command::new("media-convert");
    #[cfg(test)]
    let mut converter = {
        let mut command = Command::new("python3");
        command.arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docker/tools/media-convert.py"
        ));
        command
    };
    let mut child = converter
        .arg(mode)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let format = stderr
        .lines()
        .find_map(|l| l.strip_prefix("FORMAT:"))
        .unwrap_or(if mode == "silk2mp3" { "mp3" } else { "jpeg" })
        .to_string();
    Some((output.stdout, format))
}

// ── .dat file resolution via hardlink.db ─────────────────────────────────────

fn find_dat_via_hardlink(
    account_dir: &str,
    keys: &HashMap<String, String>,
    _chat_id: &str,
    content: &str,
    quality: ImageQuality,
) -> Option<String> {
    let hardlink_key = match keys.get("hardlink.db") {
        Some(k) => k,
        None => {
            tracing::warn!("[media:hardlink] no key for hardlink.db");
            return None;
        }
    };
    let image_md5 = match xml_attr(content, "md5") {
        Some(m) => m,
        None => {
            tracing::warn!("[media:hardlink] no md5 attr in content (len={})", content.len());
            return None;
        }
    };
    let hardlink_db = get_db_path(account_dir, "hardlink.db");

    let file_rows = query_wechat_db(
        &hardlink_db,
        hardlink_key,
        &format!(
            "SELECT file_name, dir1, dir2 FROM image_hardlink_info_v4
             WHERE md5 = '{image_md5}' LIMIT 2;"
        ),
    );
    let row = match file_rows.first() {
        Some(r) => r,
        None => {
            tracing::warn!("[media:hardlink] no hardlink row for md5={}", image_md5);
            return None;
        }
    };
    let file_name = row.get("file_name")?.as_str()?;
    let stem = file_name.strip_suffix(".dat")?;
    let stem = stem.strip_suffix("_h")
        .or_else(|| stem.strip_suffix("_t"))
        .unwrap_or(stem);
    if stem.len() != 32 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let dir1 = row.get("dir1")?.as_i64()?;
    let dir2 = row.get("dir2")?.as_i64()?;

    let dir_rows = query_wechat_db(
        &hardlink_db,
        hardlink_key,
        &format!("SELECT rowid, username FROM dir2id WHERE rowid IN ({dir1}, {dir2});"),
    );
    let dir_map: HashMap<i64, String> = dir_rows
        .iter()
        .filter_map(|r| {
            let rid = r.get("rowid")?.as_i64()?;
            let name = r.get("username")?.as_str()?.to_string();
            Some((rid, name))
        })
        .collect();

    let chat_dir = dir_map.get(&dir1)?;
    let date_dir = dir_map.get(&dir2)?;

    for base in &account_base_paths(account_dir) {
      for suffix in image_suffixes(quality) {
        let file_name = format!("{stem}{suffix}.dat");
        let dat_path = Path::new(base)
            .join("msg/attach")
            .join(chat_dir)
            .join(date_dir)
            .join("Img")
            .join(&file_name);
        if dat_path.exists() {
            return Some(dat_path.to_string_lossy().to_string());
        }
      }
    }
    tracing::warn!("[media:hardlink] .dat file not found on disk for md5={}", image_md5);
    None
}

/// Look up the file hash for a message from message_resource.db.
/// Returns the 32-char hex hash used in filenames on disk.
fn find_file_hash_via_resource_db(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
) -> Option<String> {
    let resource_key = keys.get("message_resource.db")?;
    let resource_db = get_db_path(account_dir, "message_resource.db");

    // Look up chat_id integer from ChatName2Id
    let chat_rows = query_wechat_db(
        &resource_db,
        resource_key,
        &format!(
            "SELECT rowid FROM ChatName2Id WHERE user_name = '{}' LIMIT 1;",
            chat_id.replace('\'', "''")
        ),
    );
    let chat_id_int = chat_rows.first()?.get("rowid")?.as_i64()?;

    // Query packed_info from MessageResourceInfo
    let info_rows = query_wechat_db(
        &resource_db,
        resource_key,
        &format!(
            "SELECT hex(packed_info) as hex_info FROM MessageResourceInfo
             WHERE chat_id = {chat_id_int} AND message_local_id = {local_id}
             LIMIT 1;"
        ),
    );
    let hex_info = info_rows.first()?.get("hex_info")?.as_str()?.to_string();

    let file_hash = extract_file_hash_from_packed_info(&hex_info)?;
    tracing::info!("[media:resource-db] file_hash={} for local_id={}", file_hash, local_id);
    Some(file_hash)
}

/// Look up the .dat filename from message_resource.db. The packed_info blob in
/// MessageResourceInfo contains the file hash used as the .dat filename.
fn find_dat_via_resource_db(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
    create_time: i64,
    quality: ImageQuality,
) -> Option<String> {
    let file_hash = find_file_hash_via_resource_db(account_dir, keys, chat_id, local_id)?;

    // Build path: msg/attach/<md5(chatId)>/<year-month>/Img/<hash>.dat
    let chat_hash = format!("{:x}", Md5::digest(chat_id.as_bytes()));
    let dt = chrono::DateTime::from_timestamp(create_time, 0)?;
    let year_month = dt.format("%Y-%m").to_string();

    for base in &account_base_paths(account_dir) {
        // Never satisfy a full-resolution request with a smaller variant.
        for suffix in image_suffixes(quality) {
            let dat_path = Path::new(base)
                .join("msg/attach")
                .join(&chat_hash)
                .join(&year_month)
                .join("Img")
                .join(format!("{file_hash}{suffix}.dat"));
            if dat_path.exists() {
                return Some(dat_path.to_string_lossy().to_string());
            }
        }
    }

    tracing::warn!("[media:resource-db] file not on disk yet for hash={}", file_hash);
    None
}

fn video_matches_message(data: &[u8], content: &str) -> bool {
    let Some(expected_size) = video_attr(content, "length").and_then(|n| n.parse::<u64>().ok()) else {
        return false;
    };
    if data.len() as u64 != expected_size { return false; }
    let Some(expected_md5) = video_attr(content, "md5") else { return false; };
    expected_md5.len() == 32
        && expected_md5.bytes().all(|b| b.is_ascii_hexdigit())
        && format!("{:x}", Md5::digest(data)).eq_ignore_ascii_case(&expected_md5)
}

/// Find the complete MP4 for a message. message_resource.db normally points at
/// the MP4 basename, but self-sent videos can point at the thumbnail basename
/// instead. In that case, inspect only MP4s from the message's month, discard
/// files with the wrong size without reading them, and require the message MD5.
fn find_matching_video(
    video_dir: &Path,
    preferred_hash: Option<&str>,
    content: &str,
) -> Option<Vec<u8>> {
    let expected_size = video_attr(content, "length")?.parse::<u64>().ok()?;
    let expected_md5 = video_attr(content, "md5")?;
    if expected_md5.len() != 32 || !expected_md5.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    let preferred_path = preferred_hash.map(|hash| video_dir.join(format!("{hash}.mp4")));
    if let Some(path) = preferred_path.as_ref() {
        if path.metadata().ok().map(|meta| meta.len()) == Some(expected_size) {
            if let Ok(data) = fs::read(path) {
                if video_matches_message(&data, content) {
                    return Some(data);
                }
            }
        }
    }

    let entries = fs::read_dir(video_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if preferred_path.as_ref() == Some(&path)
            || path.extension().and_then(|ext| ext.to_str()) != Some("mp4")
            || entry.metadata().ok().map(|meta| meta.len()) != Some(expected_size)
        {
            continue;
        }
        if let Ok(data) = fs::read(&path) {
            if format!("{:x}", Md5::digest(&data)).eq_ignore_ascii_case(&expected_md5) {
                return Some(data);
            }
        }
    }
    None
}

/// Get video data. Full quality returns only a complete message-matching MP4;
/// legacy and thumbnail requests may return a cached cover while transfer waits.
/// Videos are stored unencrypted at msg/video/{YYYY-MM}/{hash}.mp4
fn get_video_data(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
    create_time: i64,
    content: &str,
    quality: ImageQuality,
) -> MediaResult {
    let dt = match chrono::DateTime::from_timestamp(create_time, 0) {
        Some(dt) => dt,
        None => return unsupported(),
    };
    let year_month = dt.format("%Y-%m").to_string();

    // Try to get file hash from message_resource.db
    let file_hash = find_file_hash_via_resource_db(account_dir, keys, chat_id, local_id);

    for base in &account_base_paths(account_dir) {
        let video_dir = Path::new(base).join("msg/video").join(&year_month);

        if quality != ImageQuality::Thumbnail {
            if let Some(data) = find_matching_video(&video_dir, file_hash.as_deref(), content) {
                tracing::info!("[media:video] found message-matching mp4 for local_id={}, size={}", local_id, data.len());
                return MediaResult {
                    media_type: "video".into(),
                    data: Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &data,
                    )),
                    url: None,
                    format: "mp4".into(),
                    filename: format!("msg_{local_id}.mp4"),
                    quality: None,
                };
            }
        }

        if quality == ImageQuality::Full { continue; }

        if let Some(ref hash) = file_hash {

            // Try cover .jpg (full-size cover image)
            let cover_path = video_dir.join(format!("{hash}.jpg"));
            if cover_path.exists() {
                if let Ok(data) = fs::read(&cover_path) {
                    tracing::info!("[media:video] found cover for local_id={}", local_id);
                    return MediaResult {
                        media_type: "video".into(),
                        data: Some(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &data,
                        )),
                        url: None,
                        format: "jpeg".into(),
                        filename: format!("msg_{local_id}_cover.jpg"),
                        quality: None,
                    };
                }
            }

            // Try _thumb.jpg
            let thumb_path = video_dir.join(format!("{hash}_thumb.jpg"));
            if thumb_path.exists() {
                if let Ok(data) = fs::read(&thumb_path) {
                    tracing::info!("[media:video] found thumb for local_id={}", local_id);
                    return MediaResult {
                        media_type: "video".into(),
                        data: Some(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &data,
                        )),
                        url: None,
                        format: "jpeg".into(),
                        filename: format!("msg_{local_id}_thumb.jpg"),
                        quality: None,
                    };
                }
            }
        }
    }

    if quality != ImageQuality::Full {
        if let Some(thumb) = get_image_thumbnail(account_dir, chat_id, local_id, create_time) {
            return thumb;
        }
    }

    // Video exists but no file found on disk yet
    tracing::warn!("[media:video] no video file found for local_id={}", local_id);
    pending()
}

/// Extract the 32-char hex file hash from a MessageResourceInfo packed_info blob.
/// The blob is protobuf-encoded: field 2 (tag 0x12), length-delimited, containing
/// field 1 (tag 0x0A), 32 bytes of ASCII hex hash.
fn extract_file_hash_from_packed_info(hex_info: &str) -> Option<String> {
    let bytes = crate::tools::wechat_messages::hex_decode(hex_info)?;
    // Find the ASCII hex hash: 32 chars [0-9a-f]
    // It's at a fixed offset in the protobuf, but let's be robust and scan for it
    for window in bytes.windows(32) {
        if window.iter().all(|&b| b.is_ascii_hexdigit()) {
            let candidate = std::str::from_utf8(window).ok()?;
            // Verify it's lowercase hex (not random ASCII digits)
            if candidate.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn decrypt_and_return(dat_path: &str, image_keys: &ImageKeys, local_id: i64) -> MediaResult {
    let dat = match fs::read(dat_path) {
        Ok(d) => d,
        Err(_) => {
            tracing::warn!("[media:image] cache read failed");
            return pending();
        }
    };
    let xor_byte = match resolve_xor_byte(dat_path, &dat, image_keys) {
        Some(xb) => xb,
        None => {
            tracing::warn!("[media:image] XOR derivation failed");
            return pending();
        }
    };
    let decrypted = match decrypt_dat(&dat, &image_keys.aes_key_hex, xor_byte) {
        Some(d) => d,
        None => {
            tracing::warn!("[media:image] decryption failed");
            return pending();
        }
    };

    let (format, ext) = detect_image_format(&decrypted);

    // A different quality must not substitute for an incomplete requested image.
    if format == "wxgf" {
        if let Some((converted, cfmt)) = convert_media("wxgf2img", &decrypted) {
            let Some((converted, _)) = convert_media("normalize-image", &converted) else {
                tracing::warn!("[media:image] converted image validation failed");
                return pending();
            };
            let cext = if cfmt == "jpeg" {
                "jpg".to_string()
            } else {
                cfmt.clone()
            };
            return MediaResult {
                media_type: "image".into(),
                data: Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &converted,
                )),
                url: None,
                format: cfmt,
                filename: format!("msg_{local_id}.{cext}"),
                quality: None,
            };
        }
        tracing::warn!("[media:image] WXGF conversion failed");
        return pending();
    }

    let Some((decrypted, _)) = convert_media("normalize-image", &decrypted) else {
        tracing::warn!("[media:image] format validation failed");
        return pending();
    };

    MediaResult {
        media_type: "image".into(),
        data: Some(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &decrypted,
        )),
        url: None,
        format: format.into(),
        filename: format!("msg_{local_id}.{ext}"),
        quality: None,
    }
}

// ── Emoji ────────────────────────────────────────────────────────────────────

fn get_emoji_media(
    account_dir: &str,
    keys: &HashMap<String, String>,
    content: &str,
    _local_id: i64,
) -> MediaResult {
    let md5_val = match xml_attr(content, "md5") {
        Some(m) => m,
        None => return unsupported(),
    };

    // Look up CDN URL from emoticon.db
    if let Some(emoticon_key) = keys.get("emoticon.db") {
        let emoticon_db = get_db_path(account_dir, "emoticon.db");
        let rows = query_wechat_db(
            &emoticon_db,
            emoticon_key,
            &format!(
                "SELECT cdn_url FROM kNonStoreEmoticonTable WHERE md5 = '{md5_val}' LIMIT 1;"
            ),
        );
        if let Some(row) = rows.first() {
            if let Some(url) = row.get("cdn_url").and_then(|v| v.as_str()) {
                if !url.is_empty() {
                    return MediaResult {
                        media_type: "emoji".into(),
                        data: None,
                        url: Some(url.to_string()),
                        format: "gif".into(),
                        filename: format!("emoji_{md5_val}.gif"),
                        quality: None,
                    };
                }
            }
        }
    }

    // Fallback: extract cdnurl from message XML
    if let Some(url) = xml_attr(content, "cdnurl") {
        if url.starts_with("http") {
            return MediaResult {
                media_type: "emoji".into(),
                data: None,
                url: Some(url),
                format: "gif".into(),
                filename: format!("emoji_{md5_val}.gif"),
                quality: None,
            };
        }
    }

    MediaResult {
        media_type: "emoji".into(),
        data: None,
        url: None,
        format: "unknown".into(),
        filename: format!("emoji_{md5_val}"),
        quality: None,
    }
}

// ── Voice ────────────────────────────────────────────────────────────────────

fn get_voice_data(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
) -> MediaResult {
    // Try media_0.db, media_1.db, etc.
    let mut media_dbs: Vec<(&str, &str)> = keys
        .iter()
        .filter(|(k, _)| k.starts_with("media_") && k.ends_with(".db"))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    media_dbs.sort_by_key(|(k, _)| k.to_string());

    for (db_name, media_key) in &media_dbs {
        let media_db = get_db_path(account_dir, db_name);

        let name_rows = query_wechat_db(
            &media_db,
            media_key,
            &format!(
                "SELECT rowid FROM Name2Id WHERE user_name = '{}';",
                chat_id.replace('\'', "''")
            ),
        );
        let chat_name_id = match name_rows.first().and_then(|r| r.get("rowid")?.as_i64()) {
            Some(id) => id,
            None => continue,
        };

        let voice_rows = query_wechat_db(
            &media_db,
            media_key,
            &format!(
                "SELECT hex(voice_data) as hex_data FROM VoiceInfo
                 WHERE chat_name_id = {chat_name_id} AND local_id = {local_id}
                 LIMIT 1;"
            ),
        );
        let hex_data = match voice_rows
            .first()
            .and_then(|r| r.get("hex_data")?.as_str())
        {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => continue,
        };

        let silk_bytes = match crate::tools::wechat_messages::hex_decode(&hex_data) {
            Some(b) => b,
            None => continue,
        };

        // Try SILK → MP3 conversion
        if let Some((mp3, _)) = convert_media("silk2mp3", &silk_bytes) {
            return MediaResult {
                media_type: "voice".into(),
                data: Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &mp3,
                )),
                url: None,
                format: "mp3".into(),
                filename: format!("msg_{local_id}.mp3"),
                quality: None,
            };
        }

        // Fall back to raw SILK
        return MediaResult {
            media_type: "voice".into(),
            data: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &silk_bytes,
            )),
            url: None,
            format: "silk".into(),
            filename: format!("msg_{local_id}.silk"),
            quality: None,
        };
    }

    pending()
}

// ── File attachment ──────────────────────────────────────────────────────────

fn get_file_attachment(
    account_dir: &str,
    content: &str,
    create_time: i64,
    local_id: i64,
) -> MediaResult {
    let filename = extract_xml_tag(content, "title").unwrap_or_else(|| format!("file_{local_id}"));
    let ext = extract_xml_tag(content, "fileext").unwrap_or_default();
    // The title comes from another client, not a trusted filesystem path.
    if !safe_attachment_name(&filename) {
        return unsupported();
    }

    // Files are stored at <account>/msg/file/YYYY-MM/<filename>
    let dt = chrono::DateTime::from_timestamp(create_time, 0);
    let year_month = dt.map(|d| d.format("%Y-%m").to_string()).unwrap_or_default();

    for base in &account_base_paths(account_dir) {
        let file_path = Path::new(base)
            .join("msg/file")
            .join(&year_month)
            .join(&filename);
        if file_path.exists() {
            if let Ok(data) = fs::read(&file_path) {
                if !file_matches_message(&data, content) {
                    continue;
                }
                return MediaResult {
                    media_type: "file".into(),
                    data: Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &data,
                    )),
                    url: None,
                    format: ext,
                    filename,
                    quality: None,
                };
            }
        }
    }

    // File not yet downloaded by WeChat
    pending()
}

fn safe_attachment_name(filename: &str) -> bool {
    !filename.is_empty()
        && filename != "."
        && filename != ".."
        && !filename.contains(['/', '\\', '\0'])
}

fn file_matches_message(data: &[u8], content: &str) -> bool {
    let Some(expected_size) = extract_xml_tag(content, "totallen")
        .and_then(|n| n.parse::<u64>().ok()) else {
        return false;
    };
    if data.len() as u64 != expected_size {
        return false;
    }
    // Equal filenames and lengths can still belong to different messages.
    let Some(expected_md5) = extract_xml_tag(content, "md5") else {
        return false;
    };
    expected_md5.len() == 32
        && expected_md5.bytes().all(|b| b.is_ascii_hexdigit())
        && format!("{:x}", Md5::digest(data)).eq_ignore_ascii_case(&expected_md5)
}

// ── Public entry point ───────────────────────────────────────────────────────

fn get_image_for_quality(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
    create_time: i64,
    content: &str,
    image_keys_raw: Option<&(String, Option<u8>)>,
    quality: ImageQuality,
) -> MediaResult {
    if matches!(quality, ImageQuality::Legacy | ImageQuality::Thumbnail) {
        if let Some(thumb) = get_image_thumbnail(account_dir, chat_id, local_id, create_time) {
            return thumb;
        }
    }

    if let Some((aes_hex, xor_byte)) = image_keys_raw {
        let image_keys = ImageKeys {
            aes_key_hex: aes_hex.clone(),
            xor_byte: *xor_byte,
        };

        if let Some(dat_path) = find_dat_via_resource_db(
            account_dir, keys, chat_id, local_id, create_time, quality,
        ) {
            tracing::info!("[media] found dat via resource-db: {}", dat_path);
            return decrypt_and_return(&dat_path, &image_keys, local_id);
        }

        if let Some(dat_path) = find_dat_via_hardlink(
            account_dir, keys, chat_id, content, quality,
        ) {
            tracing::info!("[media] found dat via hardlink: {}", dat_path);
            return decrypt_and_return(&dat_path, &image_keys, local_id);
        }

        tracing::warn!(
            "[media] no dat found for local_id={}, md5={}",
            local_id, xml_attr(content, "md5").unwrap_or_default()
        );
    } else {
        tracing::warn!("[media] no image keys available for local_id={}", local_id);
    }

    pending()
}

/// Get media attachment for a message.
pub fn get_message_media(
    account_dir: &str,
    keys: &HashMap<String, String>,
    chat_id: &str,
    local_id: i64,
    image_keys_raw: Option<(String, Option<u8>)>,
    quality: ImageQuality,
) -> MediaResult {
    let (local_type, create_time, content) =
        match lookup_message_raw(account_dir, keys, chat_id, local_id) {
            Some(t) => t,
            None => {
                tracing::warn!(
                    "[media] lookup_message_raw returned None for chat_id={}, local_id={}",
                    chat_id, local_id
                );
                return unsupported();
            }
        };

    let base = (local_type & 0xFFFFFFFF) as i32;
    let sub = (local_type >> 32) as i32;
    let non_image_quality = non_image_quality(quality);

    match base {
        49 if sub == 6 => {
            // File attachment (appmsg subtype 6)
            return get_file_attachment(account_dir, &content, create_time, local_id);
        }
        3 => {
            tracing::info!(
                "[media] image msg chat_id={}, local_id={}, create_time={}, content_len={}",
                chat_id, local_id, create_time, content.len()
            );
            if quality == ImageQuality::Best {
                // Test each cache variant independently: an incomplete _h.dat
                // must not hide an intact standard image or thumbnail.
                for (variant, label) in [
                    (ImageQuality::Full, "full"),
                    (ImageQuality::Standard, "standard"),
                    (ImageQuality::Thumbnail, "thumbnail"),
                ] {
                    let mut found = get_image_for_quality(
                        account_dir, keys, chat_id, local_id, create_time, &content,
                        image_keys_raw.as_ref(), variant,
                    );
                    if found.data.is_some() {
                        found.quality = Some(label.into());
                        return found;
                    }
                }
                pending()
            } else {
                get_image_for_quality(
                    account_dir, keys, chat_id, local_id, create_time, &content,
                    image_keys_raw.as_ref(), quality,
                )
            }
        }
        43 => {
            // Video
            get_video_data(account_dir, keys, chat_id, local_id, create_time, &content, non_image_quality)
        }
        34 => {
            // Voice
            get_voice_data(account_dir, keys, chat_id, local_id)
        }
        47 => {
            // Emoji — CDN URL is included in message content, not a downloadable media
            unsupported()
        }
        _ => {
            // Other types: check for cached thumbnail
            if let Some(thumb) =
                get_image_thumbnail(account_dir, chat_id, local_id, create_time)
            {
                return thumb;
            }
            unsupported()
        }
    }
}

#[cfg(test)]
mod quality_tests {
    use super::*;

    #[test]
    fn native_group_body_preserves_sender_identity() {
        assert_eq!(native_message_body("123@chatroom", "sender", "account", "sender:\n<msg/>".into()), Some("<msg/>".into()));
        assert_eq!(native_message_body("123@chatroom", "sender", "account", "<msg/>".into()), Some("<msg/>".into()));
        assert_eq!(native_message_body("123@chatroom", "sender", "account", "other:\n<msg/>".into()), None);
        assert_eq!(native_message_body("peer", "other", "account", "<msg/>".into()), None);
        assert_eq!(native_message_body("peer", "account", "account", "<msg/>".into()), Some("<msg/>".into()));
        assert_eq!(native_message_body("filehelper", "account", "account", "<msg/>".into()), Some("<msg/>".into()));
        assert_eq!(native_message_body("filehelper", "filehelper", "account", "<msg/>".into()), None);
        assert!(!native_group_id("group@chatroom"));
        assert!(!native_group_id("123@chatroom/other"));
        assert!(native_group_id("123@chatroom"));
    }

    #[test]
    fn full_resolution_never_selects_a_thumbnail_or_mid_size() {
        assert_eq!(image_suffixes(ImageQuality::Full), &["_h"]);
        assert_eq!(image_suffixes(ImageQuality::Standard), &[""]);
        assert_eq!(image_suffixes(ImageQuality::Thumbnail), &["_t"]);
        assert_eq!(image_suffixes(ImageQuality::Best), &["_h", "", "_t"]);
        assert_eq!(ImageQuality::default(), ImageQuality::Best);
    }

    #[test]
    fn best_default_preserves_non_image_cache_behavior() {
        assert_eq!(non_image_quality(ImageQuality::Best), ImageQuality::Legacy);
        assert_eq!(non_image_quality(ImageQuality::Standard), ImageQuality::Full);
        assert_eq!(non_image_quality(ImageQuality::Full), ImageQuality::Full);
    }

    #[test]
    fn advertised_image_variants_determine_best_target() {
        assert_eq!(best_image_target("<img cdnmidimgurl=\"mid\" />"), "standard");
        assert_eq!(best_image_target("<img cdnmidimgurl=\"mid\" cdnbigimgurl=\"big\" />"), "full");
        assert_eq!(best_image_target("<img cdnmidimgurl=\"mid\" hdlength=\"123\" />"), "full");
        assert_eq!(best_image_target("<img cdnbigimgurl=\"\" cdnthumburl=\"thumb\" />"), "thumbnail");
    }

    #[test]
    fn quality_rejects_unknown_values() {
        assert_eq!(serde_json::from_str::<ImageQuality>("\"thumbnail\"").unwrap(), ImageQuality::Thumbnail);
        assert_eq!(serde_json::from_str::<ImageQuality>("\"standard\"").unwrap(), ImageQuality::Standard);
        assert_eq!(serde_json::from_str::<ImageQuality>("\"best\"").unwrap(), ImageQuality::Best);
        assert!(serde_json::from_str::<ImageQuality>("\"anything\"").is_err());
    }

    #[test]
    fn incomplete_or_wrong_file_is_not_returned() {
        let content = "<totallen>3</totallen><md5>900150983cd24fb0d6963f7d28e17f72</md5>";
        assert!(file_matches_message(b"abc", content));
        assert!(!file_matches_message(b"ab", content));
        assert!(!file_matches_message(b"xyz", content));
        assert!(!file_matches_message(b"abc", "<totallen>3</totallen>"));
    }

    #[test]
    fn incomplete_or_wrong_video_is_not_returned() {
        let content = "<msg><videomsg length=\"3\" md5=\"900150983cd24fb0d6963f7d28e17f72\" /></msg>";
        assert!(video_matches_message(b"abc", content));
        assert!(!video_matches_message(b"ab", content));
        assert!(!video_matches_message(b"xyz", content));
        assert!(!video_matches_message(b"abc", "<msg><videomsg length=\"3\" /></msg>"));
        assert!(video_matches_message(
            b"abc",
            "<msg><videomsg rawlength=\"9\" rawmd5=\"00000000000000000000000000000000\" length=\"3\" md5=\"900150983cd24fb0d6963f7d28e17f72\" /></msg>"
        ));
    }

    #[test]
    fn video_fallback_uses_exact_identity_when_resource_hash_is_a_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("thumbnail-hash.mp4"), b"xyz").unwrap();
        fs::write(dir.path().join("actual-video-hash.mp4"), b"abc").unwrap();
        fs::write(dir.path().join("same-size-wrong-video.mp4"), b"def").unwrap();
        let content = "<msg><videomsg length=\"3\" md5=\"900150983cd24fb0d6963f7d28e17f72\" /></msg>";

        assert_eq!(
            find_matching_video(dir.path(), Some("thumbnail-hash"), content),
            Some(b"abc".to_vec())
        );
    }

    #[test]
    fn video_fallback_rejects_files_without_an_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("same-size-wrong-video.mp4"), b"def").unwrap();
        let content = "<msg><videomsg length=\"3\" md5=\"900150983cd24fb0d6963f7d28e17f72\" /></msg>";

        assert_eq!(find_matching_video(dir.path(), None, content), None);
    }

    #[test]
    fn attachment_title_is_a_single_filename() {
        assert!(safe_attachment_name("test image.png"));
        for name in ["", ".", "..", "../file", "/etc/passwd", "folder\\file", "a\0b"] {
            assert!(!safe_attachment_name(name));
        }
    }
}

#[cfg(test)]
mod decryption_tests {
    use super::*;
    use std::io::Write;

    const JPEG: &[u8] = include_bytes!("../../../../tests/fixtures/synthetic.jpg");
    const AES_KEY: &str = "0123456789abcdef0123456789abcdef";
    const XOR: u8 = 0x63;

    fn fixture(plain: &[u8], xor_size: usize) -> Vec<u8> {
        let head_size = 64usize;
        let mut child = Command::new("openssl")
            .args(["enc", "-aes-128-ecb", "-K"])
            .arg(hex_encode(&AES_KEY.as_bytes()[..16]))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&plain[..head_size])
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let mut dat = DAT_MAGIC.to_vec();
        dat.extend_from_slice(&(head_size as u32).to_le_bytes());
        dat.extend_from_slice(&(xor_size as u32).to_le_bytes());
        dat.push(0);
        dat.extend_from_slice(&output.stdout);
        dat.extend_from_slice(&plain[head_size..plain.len() - xor_size]);
        dat.extend(plain[plain.len() - xor_size..].iter().map(|b| b ^ XOR));
        dat
    }

    fn has_runtime_codec() -> bool {
        let available = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !available {
            eprintln!("JPEG integration fixture requires FFmpeg");
        }
        available
    }

    #[test]
    fn jpeg_dat_recovers_without_saved_xor_and_normalizes_trailers() {
        if !has_runtime_codec() {
            return;
        }
        for trailing in [0, 24, 191] {
            let mut plain = JPEG.to_vec();
            plain.extend(vec![b'X'; trailing]);
            let dat = fixture(&plain, 1024);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("image.dat");
            fs::write(&path, &dat).unwrap();
            for saved in [None, Some(XOR)] {
                let keys = ImageKeys {
                    aes_key_hex: AES_KEY.into(),
                    xor_byte: saved,
                };
                assert_eq!(
                    resolve_xor_byte(path.to_str().unwrap(), &dat, &keys),
                    Some(XOR)
                );
                let result = decrypt_and_return(path.to_str().unwrap(), &keys, 42);
                assert_eq!(result.media_type, "image");
                assert_eq!(result.format, "jpeg");
                let bytes = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    result.data.unwrap(),
                )
                .unwrap();
                assert_eq!(bytes, JPEG);
                assert_eq!(fs::read(&path).unwrap(), dat); // Cache bytes remain untouched.
            }
        }
    }

    #[test]
    fn derives_from_sibling_thumbnail_with_trailing_data() {
        if !has_runtime_codec() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.dat");
        let mut wxgf = vec![0; 200];
        wxgf[..4].copy_from_slice(b"wxgf");
        let dat = fixture(&wxgf, 100);
        fs::write(&path, &dat).unwrap();
        let mut thumbnail = JPEG.to_vec();
        thumbnail.extend(vec![b'X'; 24]);
        fs::write(dir.path().join("image_t.dat"), fixture(&thumbnail, 1024)).unwrap();
        let keys = ImageKeys {
            aes_key_hex: AES_KEY.into(),
            xor_byte: None,
        };
        assert_eq!(
            resolve_xor_byte(path.to_str().unwrap(), &dat, &keys),
            Some(XOR)
        );
    }

    #[test]
    fn invalid_decryption_returns_pending_without_image_bytes() {
        if !has_runtime_codec() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.dat");
        fs::write(&path, fixture(JPEG, 1024)).unwrap();
        for keys in [
            ImageKeys {
                aes_key_hex: AES_KEY.into(),
                xor_byte: Some(XOR ^ 1),
            },
            ImageKeys {
                aes_key_hex: "bad".into(),
                xor_byte: Some(XOR),
            },
            ImageKeys {
                aes_key_hex: "ffffffffffffffffffffffffffffffff".into(),
                xor_byte: Some(XOR),
            },
        ] {
            let result = decrypt_and_return(path.to_str().unwrap(), &keys, 42);
            assert_eq!(result.media_type, "pending");
            assert!(result.data.is_none());
        }
        let mut truncated = JPEG[..JPEG.len() - 100].to_vec();
        truncated.extend_from_slice(&[0xff, 0xd9]);
        fs::write(&path, fixture(&truncated, 1024)).unwrap();
        let keys = ImageKeys {
            aes_key_hex: AES_KEY.into(),
            xor_byte: None,
        };
        assert_eq!(
            decrypt_and_return(path.to_str().unwrap(), &keys, 42).media_type,
            "pending"
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            decrypt_and_return(path.to_str().unwrap(), &keys, 42).media_type,
            "pending"
        );
    }

    #[test]
    fn invalid_dat_lengths_and_short_keys_do_not_panic() {
        assert_eq!(decrypt_dat_head(&[], AES_KEY), None);
        assert_eq!(derive_xor_byte(&[], &[0xff, 0xd8]), None);
        assert_eq!(
            get_image_for_quality(
                "fixture",
                &HashMap::new(),
                "peer",
                42,
                0,
                "",
                None,
                ImageQuality::Full
            )
            .media_type,
            "pending"
        );
        let mut dat = fixture(JPEG, 1024);
        assert!(decrypt_dat(&dat, "bad", XOR).is_none());
        dat[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decrypt_dat(&dat, AES_KEY, XOR).is_none());
        dat[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decrypt_dat_head(&dat, AES_KEY).is_none());
    }
}
