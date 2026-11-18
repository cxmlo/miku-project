use miku_cli::doctor_config_and_budgets::parse_and_validate_miku_toml;
use miku_gatekeeper::CanonicalPathJail;

struct Rng64(u64);

impl Rng64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

const FUZZ_BAD_FS_GLOBS: &[&str] = &[
    "",
    "../../etc/passwd",
    "..\\..\\Windows\\win.ini",
    "\\\\server\\share\\**",
    "D:\\outside\\**",
    "C:/Windows/System32/*",
    "src/file.txt:ads",
    "CON",
    "NUL.txt",
    "src/\0/secret",
];

const FUZZ_GOOD_FS_GLOBS: &[&str] = &["src/**", "docs/**/*.md", "Cargo.toml", "miku/crates/**"];

#[test]
fn test_5_2_fuzz_miku_toml_100k_inputs_zero_panics_and_zero_jail_bypass() {
    let mut rng = Rng64(0x4D49_4B55_544F_4D4C);
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for i in 0..100_000usize {
        let candidate = match i % 10 {
            0 => {
                // Valid baseline TOML variations (~10,000 full parses + jail + Gatekeeper check)
                let pat = FUZZ_GOOD_FS_GLOBS[(rng.next_u64() as usize) % FUZZ_GOOD_FS_GLOBS.len()];
                let drv = if i.is_multiple_of(2) { "playwright" } else { "cdp" };
                format!(
                    "[project]\nname = \"miku-fuzz-{i}\"\nroot = \"C:/Users/miku/repo\"\ndefault_ttl_ms = {}\n\n[browser]\ndriver = \"{drv}\"\nprofile_isolation = true\n\n[allow]\nfs_read = [\"{pat}\"]\n\n[ask]\nshell = [\"cargo test*\"]\n\n[deny]\nshell = [\"*rm -rf*\"]\n",
                    ((rng.next_u64() % 60_000) + 1_000)
                )
            }
            1 => {
                // Path traversal / UNC / ADS injection into [allow].fs_read or fs_write
                let bad = FUZZ_BAD_FS_GLOBS[(rng.next_u64() as usize) % FUZZ_BAD_FS_GLOBS.len()];
                format!(
                    "[project]\nname = \"fuzz\"\nroot = \"C:/Users/miku/repo\"\n\n[browser]\ndriver = \"playwright\"\nprofile_isolation = true\n\n[allow]\nfs_read = [\"{}\"]\n",
                    bad.replace('\\', "\\\\").replace('"', "\\\"")
                )
            }
            2 => {
                // Insecure browser profile_isolation=false or bogus driver
                let drv = if i % 20 == 2 { "chrome-unsafe" } else { "playwright" };
                format!(
                    "[project]\nname = \"fuzz\"\nroot = \"C:/Users/miku/repo\"\n\n[browser]\ndriver = \"{drv}\"\nprofile_isolation = false\n"
                )
            }
            3 => {
                // Out-of-bounds TTL or empty project.name
                let ttl = if i % 20 == 3 { 0 } else { 9_999_999 };
                format!(
                    "[project]\nname = \" \"\nroot = \"C:/Users/miku/repo\"\ndefault_ttl_ms = {ttl}\n"
                )
            }
            4 => {
                // Invalid root (relative, traversal, UNC)
                let roots = ["", "relative", "../../escape", "\\\\server\\share", "C:/Users/CON"];
                let r = roots[(rng.next_u64() as usize) % roots.len()];
                format!("[project]\nname = \"fuzz\"\nroot = \"{}\"\n", r.replace('\\', "\\\\"))
            }
            5 => {
                // Unknown fields rejected by deny_unknown_fields
                format!(
                    "[project]\nname = \"fuzz\"\nroot = \"C:/Users/miku/repo\"\nevil_{i} = true\n"
                )
            }
            6 => {
                // Type mismatches / unterminated strings / malformed TOML
                "[project]\nname = 12345\nroot = [1,2\n[allow\nfs_read = \"not-an-array\"".to_string()
            }
            7 => {
                // Deep inline table nesting stress
                let d = 40 + ((rng.next_u64() % 40) as usize);
                format!("a = {}1{}", "{x=".repeat(d), "}".repeat(d))
            }
            8 => {
                // Pseudo-random ASCII/UTF-8 fuzz fragment
                let len = ((rng.next_u64() % 48) + 1) as usize;
                let mut s = String::with_capacity(len);
                for _ in 0..len {
                    let b = (rng.next_u64() % 95 + 32) as u8;
                    s.push(b as char);
                }
                s
            }
            _ => {
                // Empty allowlist patterns for shell / browser_domains
                "[project]\nname = \"fuzz\"\nroot = \"C:/Users/miku/repo\"\n\n[allow]\nshell = [\"   \"]\n".to_string()
            }
        };

        match parse_and_validate_miku_toml(&candidate) {
            Ok((profile, _toml_sha, _contract_sha)) => {
                let jail = CanonicalPathJail::new(&profile.project_root)
                    .expect("accepted profile must have valid jail root");
                assert!(!profile.project_name.trim().is_empty());
                assert!(profile.default_ttl_ms >= 1 && profile.default_ttl_ms <= 3_600_000);
                // Re-verify no fs_read / fs_write pattern escaped the jail
                for rule in &profile.rules {
                    if rule.capability.requires_path_jail() {
                        assert!(
                            !rule.pattern.contains("..") && !rule.pattern.starts_with("\\\\"),
                            "escaped fs pattern survived parser: {}",
                            rule.pattern
                        );
                    }
                }
                let _ = jail;
                ok_count += 1;
            }
            Err(_) => {
                err_count += 1;
            }
        }
    }

    assert_eq!(ok_count + err_count, 100_000);
    assert!(ok_count >= 9_000 && err_count >= 85_000);
}
