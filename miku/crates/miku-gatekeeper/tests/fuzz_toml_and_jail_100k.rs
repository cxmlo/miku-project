use miku_gatekeeper::{wildcard_match, CanonicalPathJail, GatekeeperEngine};

struct Rng64(u64);

impl Rng64 {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.0 = x;
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = (self.next_u64() as usize) % items.len();
        &items[idx]
    }
}

const JAIL_SEEDS: &[&str] = &[
    "",
    "   ",
    ".",
    "..",
    "../",
    "..\\",
    "../../etc/passwd",
    "..\\..\\Windows\\System32\\cmd.exe",
    "src/../../outside.txt",
    "src\\..\\..\\Windows\\win.ini",
    "\\\\server\\share\\file.txt",
    "//server/share/file.txt",
    "\\\\?\\C:\\Users\\outside.txt",
    "\\\\?\\UNC\\server\\share",
    "D:\\other_drive\\secret.txt",
    "d:/secret.txt",
    "C:/Windows/System32/drivers/etc/hosts",
    "/etc/shadow",
    "foo.txt:hidden_stream",
    "src/lib.rs:Zone.Identifier",
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM9.txt",
    "LPT1 ",
    "src/CON.rs",
    "src/NUL ",
    "src/trailing...",
    "src/trailing   ",
    "src\0lib.rs",
    "src/\x07bell.rs",
    "src/lib.rs",
    "C:\\Users\\miku\\repo\\src\\main.rs",
    "c:/users/miku/repo/./src/../Cargo.toml",
];

const SEGMENT_TOKENS: &[&str] = &[
    "src",
    "crates",
    "miku",
    ".",
    "..",
    "...",
    " ",
    "a.txt",
    "lib.rs",
    "CON",
    "NUL",
    "COM1",
    "LPT9",
    "foo:bar",
    "\0",
    "\n",
    "\\\\",
    "//",
    "D:",
    "C:",
    "__fuzz__",
];

fn assert_strictly_confined(jail: &CanonicalPathJail, canon: &str) {
    let root = jail.canonical_root();
    let lower_root = root.to_ascii_lowercase();
    let lower_canon = canon.to_ascii_lowercase();
    assert!(
        lower_canon == lower_root || lower_canon.starts_with(&format!("{}/", lower_root)),
        "path escape detected: canon='{}' escaped jail='{}'",
        canon,
        root
    );
    assert!(
        !canon.split('/').any(|seg| seg == ".."),
        "unescaped dotdot segment in '{}'",
        canon
    );
    assert!(!canon.contains("\\\\"), "unc backslash in '{}'", canon);
    assert!(!canon.bytes().any(|b| b == 0 || b < 0x20), "ctrl byte in '{}'", canon);
}

#[test]
fn test_5_2_fuzz_canonical_path_jail_100k_inputs_zero_panics_zero_escapes() {
    let win_jail = CanonicalPathJail::new(r"C:\Users\miku\repo").expect("win jail");
    let posix_jail = CanonicalPathJail::new("/workspace/miku").expect("posix jail");

    let mut rng = Rng64(0x5245_475F_4A41_494C);
    let mut confined_ok = 0usize;
    let mut rejected_err = 0usize;

    for i in 0..100_000usize {
        let mut candidate = if let Some(&seed) = JAIL_SEEDS.get(i) {
            seed.to_string()
        } else {
            let depth = ((rng.next_u64() % 6) + 1) as usize;
            let mut s = String::with_capacity(64);
            if rng.next_u64().is_multiple_of(5) {
                s.push_str(rng.pick(&["C:/Users/miku/repo/", "C:\\Users\\miku\\repo\\", "D:\\", "\\\\", "/etc/", "C:/Users/miku/repo/../other/"]));
            }
            for step in 0..depth {
                if step > 0 {
                    s.push(if rng.next_u64().is_multiple_of(2) { '/' } else { '\\' });
                }
                s.push_str(rng.pick(SEGMENT_TOKENS));
            }
            s
        };

        // Inject deterministic byte mutations into ~14% of cases
        if i.is_multiple_of(7) && !candidate.is_empty() {
            let extra = (rng.next_u64() % 128) as u8 as char;
            candidate.push(extra);
        }

        let jail = if i.is_multiple_of(2) { &win_jail } else { &posix_jail };
        match jail.resolve_and_confine(&candidate) {
            Ok(canon) => {
                assert_strictly_confined(jail, &canon);
                confined_ok += 1;
            }
            Err(_) => {
                rejected_err += 1;
            }
        }
    }

    assert_eq!(confined_ok + rejected_err, 100_000);
    assert!(confined_ok > 0 && rejected_err > 0);
}

#[test]
fn test_5_2_fuzz_jail_root_and_wildcard_matcher_adversarial_corpus() {
    let bad_roots = [
        "",
        "   ",
        "relative/path",
        r"\\server\share",
        "//server/share",
        "C:/../escape",
        "C:/Users/CON",
        "/etc/NUL",
        "C:/foo:ads",
        "C:/\0/bad",
    ];
    for r in bad_roots {
        assert!(
            CanonicalPathJail::new(r).is_err(),
            "root '{}' should have been rejected",
            r
        );
    }

    // ReDoS / exponential backtracking guard on wildcard_match with dense `*` stars
    let pathological_pat = "*a*a*a*a*a*a*a*a*a*a*a*b";
    let pathological_txt = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert!(!wildcard_match(pathological_pat, pathological_txt));
    assert!(!GatekeeperEngine::compute_request_hash("agent", miku_gatekeeper::Capability::FsRead, "src/lib.rs", "").is_empty());
}
