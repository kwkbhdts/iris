#[derive(Debug)]
pub struct Identity {
    pub product: String,
    pub numeric: [u16; 4],
    pub file: String,
    pub origin: &'static str,
    pub run: String,
    pub attempt: String,
    pub commit: String,
    pub tree: String,
    pub label: String,
}

/// 数値版は切り捨てず、CI の識別子は独立した文字列でも保持する。
pub fn identity(
    product: &str,
    ci: Option<(&str, &str, &str)>,
    commit: &str,
    tree: &str,
) -> Result<Identity, String> {
    let core = product.split(['-', '+']).next().unwrap();
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3 {
        return Err("Cargo version must have three numeric components".into());
    }
    let mut numeric = [0u16; 4];
    for (slot, part) in numeric.iter_mut().zip(parts) {
        *slot = part
            .parse()
            .map_err(|_| "Cargo version exceeds a 16-bit component")?;
    }
    if commit != "unknown"
        && !([40, 64].contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Invalid commit ID".into());
    }
    if !["clean", "dirty", "unknown"].contains(&tree) {
        return Err("Invalid source tree state".into());
    }
    let (origin, run, attempt) = if let Some((number, run, attempt)) = ci {
        numeric[3] = number
            .parse()
            .map_err(|_| "CI run number exceeds 65535; revise version policy explicitly")?;
        if numeric[3] == 0 || commit == "unknown" || tree != "clean" {
            return Err("CI requires a positive run number and a known clean commit".into());
        }
        for value in [run, attempt] {
            if value.parse::<u64>().ok().filter(|n| *n > 0).is_none()
                || !value.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("CI run ID and attempt must be positive decimal integers".into());
            }
        }
        ("github", run, attempt)
    } else {
        ("local", "none", "none")
    };
    let file = numeric.map(|n| n.to_string()).join(".");
    let label = format!("product={product} file={file} origin={origin} run={run} attempt={attempt} commit={commit} tree={tree}");
    Ok(Identity {
        product: product.into(),
        numeric,
        file,
        origin,
        run: run.into(),
        attempt: attempt.into(),
        commit: commit.into(),
        tree: tree.into(),
        label,
    })
}

/// UTF-16 の終端付き文字列を Windows リソース用に直列化する。
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// リソース内の各構造を DWORD 境界へそろえる。
fn align(bytes: &mut Vec<u8>) {
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

/// VERSIONINFO の入れ子を作り、長さの単位を明示する。
fn block(key: &str, kind: u16, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend(value_len.to_le_bytes());
    out.extend(kind.to_le_bytes());
    out.extend(wide(key));
    align(&mut out);
    out.extend(value);
    for child in children {
        align(&mut out);
        out.extend(child);
    }
    let length = u16::try_from(out.len()).expect("VERSIONINFO block too large");
    out[..2].copy_from_slice(&length.to_le_bytes());
    out
}

/// 言語中立の RT_VERSION を .res レコードに包む。
pub fn version_resource(info: &Identity, bin: &str) -> Vec<u8> {
    let strings = [
        ("FileDescription", format!("Iris ({bin})")),
        ("FileVersion", info.file.clone()),
        ("ProductName", "Iris".into()),
        ("ProductVersion", info.product.clone()),
        ("OriginalFilename", format!("{bin}.exe")),
        ("InternalName", bin.into()),
        ("Comments", info.label.clone()),
        ("BuildOrigin", info.origin.into()),
        ("BuildRunId", info.run.clone()),
        ("BuildAttempt", info.attempt.clone()),
        ("Commit", info.commit.clone()),
        ("SourceTree", info.tree.clone()),
        (
            "PrivateBuild",
            if info.origin == "local" {
                info.label.clone()
            } else {
                String::new()
            },
        ),
    ]
    .map(|(key, value)| {
        let bytes = wide(&value);
        block(key, 1, &bytes, u16::try_from(bytes.len() / 2).unwrap(), &[])
    });
    let table = block("040904B0", 1, &[], 0, &strings);
    let text = block("StringFileInfo", 1, &[], 0, &[table]);
    let translation = block("Translation", 0, &[9, 4, 0xB0, 4], 4, &[]);
    let vars = block("VarFileInfo", 1, &[], 0, &[translation]);
    let [major, minor, patch, build] = info.numeric.map(u32::from);
    let fixed: Vec<u8> = [
        0xFEEF04BDu32,
        0x00010000,
        major << 16 | minor,
        patch << 16 | build,
        major << 16 | minor,
        patch << 16,
        0x3F,
        if info.origin == "local" { 8 } else { 0 },
        0x00040004,
        1,
        0,
        0,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    let data = block("VS_VERSION_INFO", 0, &fixed, 52, &[text, vars]);
    let mut res = Vec::new();
    res.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
    res.extend(32u32.to_le_bytes());
    for n in [0xFFFFu16, 16, 0xFFFF, 1] {
        res.extend(n.to_le_bytes());
    }
    res.extend(0u32.to_le_bytes());
    res.extend(0x30u16.to_le_bytes());
    res.extend(0u16.to_le_bytes());
    res.extend([0u8; 8]);
    res.extend(data);
    align(&mut res);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    const SHA: &str = "0123456789012345678901234567890123456789";

    /// 16ビットの境界を通し、丸めと CI のゼロ番号を拒否する。
    #[test]
    fn numeric_bounds_are_explicit() {
        assert_eq!(
            identity(
                "65535.65535.65535",
                Some(("65535", "38241299075", "1")),
                SHA,
                "clean"
            )
            .unwrap()
            .numeric,
            [65535; 4]
        );
        for product in ["65536.1.0", "0.65536.0", "0.1.65536"] {
            assert!(identity(product, None, SHA, "clean").is_err());
        }
        for number in ["0", "65536", "-1", "x"] {
            assert!(identity("0.1.0", Some((number, "1", "1")), SHA, "clean").is_err());
        }
    }

    /// 同じ連番の別実行・再実行とローカルを識別できる。
    #[test]
    fn identity_does_not_rely_on_workflow_number() {
        let a = identity("0.1.0", Some(("42", "100", "1")), SHA, "clean").unwrap();
        let b = identity("0.1.0", Some(("42", "101", "1")), SHA, "clean").unwrap();
        let c = identity("0.1.0", Some(("42", "100", "2")), SHA, "clean").unwrap();
        assert_eq!(a.file, b.file);
        assert_ne!(a.label, b.label);
        assert_ne!(a.label, c.label);
        let local = identity("0.1.0", None, SHA, "dirty").unwrap();
        assert_eq!(local.numeric[3], 0);
        assert!(local.label.contains("origin=local"));
        assert!(local.label.contains("tree=dirty"));
        assert!(identity("0.1.0", None, "unknown", "unknown").is_ok());
    }

    /// 不完全な CI 情報や変更済みツリーは配布識別子にしない。
    #[test]
    fn incomplete_ci_is_rejected() {
        for (run, attempt) in [("", "1"), ("1", "0"), ("abc", "2")] {
            assert!(identity("0.1.0", Some(("1", run, attempt)), SHA, "clean").is_err());
        }
        assert!(identity("0.1.0", Some(("1", "1", "1")), "unknown", "clean").is_err());
        assert!(identity("0.1.0", Some(("1", "1", "1")), SHA, "dirty").is_err());
    }

    /// 同じ入力から同じリソースを作り、Cargo の接尾辞も保持する。
    #[test]
    fn resources_are_deterministic() {
        let info = identity("0.1.0-rc.1+test", None, SHA, "clean").unwrap();
        let a = version_resource(&info, "iris");
        assert_eq!(a, version_resource(&info, "iris"));
        assert_ne!(a, version_resource(&info, "iris-focus-probe"));
        assert!(a
            .windows(wide(&info.product).len())
            .any(|v| v == wide(&info.product)));
    }
}
