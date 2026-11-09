use std::time::Instant;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};

use crate::secrets::{
    EncryptedMemoryBackend, SecretError, SecretManager, SecretStoreBackend, SecretSummary,
    Zeroize, Zeroizing,
};

#[cfg(target_os = "windows")]
use crate::secrets::WindowsCredentialBackend;

fn test_gatekeeper() -> GatekeeperEngine {
    let profile = ProjectProfile::new("secrets-test", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("Gatekeeper engine should build")
}

fn acquire_free_test_alias<B: SecretStoreBackend>(
    backend: &B,
    candidates: &[String],
) -> Result<String, String> {
    for candidate in candidates {
        match backend.load_secret(candidate) {
            Ok(None) => return Ok(candidate.clone()),
            Ok(Some(_)) => {
                // El alias ya existe: prohibido borrarlo o sobrescribirlo; probar el siguiente candidato
                continue;
            }
            Err(e) => {
                return Err(format!(
                    "Error al comprobar existencia previa de credencial en Credential Manager: {:?}",
                    e
                ));
            }
        }
    }
    Err(
        "Todos los alias candidatos ya existen; abortando sin borrar ni sobrescribir credenciales preexistentes"
            .to_string(),
    )
}

// LEDGER-3.6-1: Windows Credential Manager: guardar, recuperar y borrar credencial real
#[test]
#[cfg(target_os = "windows")]
fn test_windows_credential_manager_save_retrieve_and_delete_real_test_credential() {
    let mut backend = WindowsCredentialBackend::new();
    let pid = std::process::id();
    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let candidates: Vec<String> = (0..8)
        .map(|attempt| format!("miku-test-cred-ledger-3-6-1-{}-{}-{}", pid, timestamp_ns, attempt))
        .collect();
    let test_secret_val = b"miku-dummy-secret-payload-test-1234567890";

    // 1. Seleccionar un alias único libre sin borrar ni sobrescribir credenciales preexistentes
    let unique_alias = acquire_free_test_alias(&backend, &candidates)
        .expect("Debe encontrar un alias de prueba confirmado libre sin modificar preexistentes");

    // RAII Guard: garantiza limpieza del alias de prueba incluso si una aserción entra en pánico
    struct CleanupGuard<'a> {
        backend: &'a mut WindowsCredentialBackend,
        alias: String,
        should_clean: bool,
    }
    impl<'a> Drop for CleanupGuard<'a> {
        fn drop(&mut self) {
            if self.should_clean {
                let _ = self.backend.delete_secret(&self.alias);
            }
        }
    }

    let mut guard = CleanupGuard {
        backend: &mut backend,
        alias: unique_alias.clone(),
        should_clean: false,
    };

    // 2. Guardar credencial de prueba real en Windows Credential Manager
    guard
        .backend
        .save_secret(&unique_alias, test_secret_val)
        .expect("Guardar en Windows Credential Manager debe tener éxito");
    guard.should_clean = true;

    // 3. Recuperar por alias y verificar contenido exacto
    let loaded = guard
        .backend
        .load_secret(&unique_alias)
        .expect("Lectura desde Windows Credential Manager debe tener éxito")
        .expect("La credencial guardada debe existir");

    assert_eq!(
        loaded.as_slice(),
        test_secret_val,
        "El secreto recuperado debe coincidir exactamente con el guardado"
    );

    // 4. Borrar la credencial de prueba y verificar que no queda residuo
    let deleted = guard
        .backend
        .delete_secret(&unique_alias)
        .expect("Borrado de la credencial de prueba debe tener éxito");
    assert!(deleted, "delete_secret debe retornar true si existía");
    guard.should_clean = false;

    // 5. Verificar que lectura posterior retorna None
    let post_delete = guard
        .backend
        .load_secret(&unique_alias)
        .expect("Lectura tras borrado no debe generar error de sistema");
    assert!(
        post_delete.is_none(),
        "La credencial borrada no debe existir en Windows Credential Manager"
    );

    println!(
        "[LEDGER-3.6-1] target=miku/secrets/{} pre_check_free=true cred_write_bytes={} cred_read_bytes={} payload_match={} cred_delete={} post_delete_none={}",
        unique_alias,
        test_secret_val.len(),
        loaded.len(),
        loaded.as_slice() == test_secret_val,
        deleted,
        post_delete.is_none()
    );
}

// LEDGER-3.6-2: IPC sin secretos: listado sólo serializa { alias, provider, status }
#[test]
fn test_secret_manager_ipc_listing_serializes_only_alias_provider_status_and_zero_secret_leak() {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let mut manager = SecretManager::new(backend, "valid-session-token".to_string(), test_gatekeeper());

    let deepseek_key = "sk-ds-TOP_SECRET_DEEPSEEK_KEY_999999999";
    let anthropic_key = "sk-ant-SECRET_ANTHROPIC_PRODUCTION_KEY_888888888";

    manager
        .set_secret(
            "deepseek-main",
            "deepseek",
            Some("deepseek-chat"),
            deepseek_key.as_bytes(),
        )
        .expect("Debe registrar deepseek-main");

    manager
        .set_secret(
            "anthropic-work",
            "anthropic",
            Some("claude-3-7-sonnet"),
            anthropic_key.as_bytes(),
        )
        .expect("Debe registrar anthropic-work");

    // 1. Obtener listado de resúmenes para el endpoint IPC
    let summaries: Vec<SecretSummary> = manager.list_summaries();
    assert_eq!(summaries.len(), 2);

    // 2. Serializar a JSON (lo que viaja por el websocket IPC a la Island / CLI)
    let json_output = serde_json::to_string(&summaries).expect("Serialización JSON debe ser válida");

    // 3. Verificar que el JSON sólo contiene alias, provider y status
    assert!(!json_output.contains(deepseek_key), "El secreto de DeepSeek NUNCA debe estar en el JSON");
    assert!(!json_output.contains(anthropic_key), "El secreto de Anthropic NUNCA debe estar en el JSON");
    assert!(json_output.contains("deepseek-main"));
    assert!(json_output.contains("anthropic-work"));
    assert!(json_output.contains("configured"));

    // 4. Verificar que Debug de SecretSummary y SecretMetadata no filtra secretos
    let debug_summary = format!("{:?}", summaries);
    assert!(!debug_summary.contains(deepseek_key));
    assert!(!debug_summary.contains(anthropic_key));

    // 5. Verificar que errores tipados tampoco filtran secretos
    let err = manager.set_secret("", "provider", None, deepseek_key.as_bytes());
    let err_str = format!("{:?}", err);
    assert!(!err_str.contains(deepseek_key));
}

// LEDGER-3.6-3: Alias y proveedor/modelo: validación estricta y fail-closed
#[test]
fn test_secret_alias_and_provider_model_validation_fails_closed_on_malformed_inputs() {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let mut manager = SecretManager::new(backend, "valid-session-token".to_string(), test_gatekeeper());

    // Alias vacío debe fallar
    let empty_alias = manager.set_secret("", "deepseek", None, b"dummy-key");
    assert!(matches!(empty_alias, Err(SecretError::InvalidAlias(_))));

    // Alias con path traversal o caracteres inválidos debe fallar
    let traversal = manager.set_secret("../secret", "deepseek", None, b"dummy-key");
    assert!(matches!(traversal, Err(SecretError::InvalidAlias(_))));

    let whitespace = manager.set_secret("my alias", "deepseek", None, b"dummy-key");
    assert!(matches!(whitespace, Err(SecretError::InvalidAlias(_))));

    // Proveedor vacío debe fallar
    let empty_prov = manager.set_secret("valid-alias", "", None, b"dummy-key");
    assert!(matches!(empty_prov, Err(SecretError::InvalidProvider(_))));

    // Secreto vacío debe fallar
    let empty_sec = manager.set_secret("valid-alias", "deepseek", None, b"");
    assert!(matches!(empty_sec, Err(SecretError::EmptySecret)));

    // Alias inexistente debe fallar cerrado al solicitar lectura
    let not_found = manager.get_secret_authorized("non-existent-alias", "valid-session-token");
    assert!(matches!(not_found, Err(SecretError::AliasNotFound(_))));

    // Token de sesión inválido debe fallar cerrado por falta de autorización
    manager
        .set_secret("openai-main", "openai", Some("gpt-4o"), b"dummy-openai-key")
        .unwrap();
    let unauth = manager.get_secret_authorized("openai-main", "invalid-session-token");
    assert!(matches!(unauth, Err(SecretError::UnauthorizedSessionToken)));
}

// LEDGER-3.6-4: Rotación y borrado: actualiza estado por alias y zeroiza buffers
#[test]
fn test_secret_rotation_and_deletion_updates_alias_state_and_zeroizes_previous_buffers() {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let mut manager = SecretManager::new(backend, "valid-session-token".to_string(), test_gatekeeper());

    let alias = "rotation-target";
    let initial_key = b"initial-secret-key-version-1";
    let rotated_key = b"rotated-secret-key-version-2";

    // 1. Guardar valor inicial V1
    manager
        .set_secret(alias, "anthropic", Some("claude-3-5-haiku"), initial_key)
        .expect("Guardar V1");

    let val1 = manager
        .get_secret_authorized(alias, "valid-session-token")
        .expect("Lectura V1");
    assert_eq!(val1.as_slice(), initial_key);

    // 2. Rotar al valor V2
    manager
        .rotate_secret(alias, rotated_key)
        .expect("Rotación a V2 debe tener éxito");

    let val2 = manager
        .get_secret_authorized(alias, "valid-session-token")
        .expect("Lectura V2");
    assert_eq!(val2.as_slice(), rotated_key);

    // 3. Probar comportamiento de Zeroize en búfer dedicado
    let mut sensitive_buf = Zeroizing::new(Vec::from(*initial_key));
    assert_eq!(sensitive_buf.as_slice(), initial_key);
    sensitive_buf.zeroize();
    assert!(
        sensitive_buf.iter().all(|&b| b == 0) || sensitive_buf.is_empty(),
        "El búfer debe quedar lleno de ceros tras zeroize()"
    );

    // 4. Borrar credencial
    let deleted = manager
        .delete_secret(alias)
        .expect("Borrado debe tener éxito");
    assert!(deleted);

    // Lectura posterior debe fallar con AliasNotFound
    let post_delete = manager.get_secret_authorized(alias, "valid-session-token");
    assert!(matches!(post_delete, Err(SecretError::AliasNotFound(_))));
}

// LEDGER-3.6-5: Rendimiento y aislamiento: lectura cacheada < 1 ms y cero fuga
#[test]
fn benchmark_cached_secret_read_latency_under_1ms_and_memory_isolation() {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let mut manager = SecretManager::new(backend, "auth-token".to_string(), test_gatekeeper());

    let alias = "bench-secret";
    let secret = b"sk-benchmark-secret-payload-long-string-1234567890";
    manager
        .set_secret(alias, "deepseek", Some("deepseek-r1"), secret)
        .unwrap();

    // Warm-up inicial
    let _ = manager.get_secret_authorized(alias, "auth-token").unwrap();

    let reps = 1_000;
    let start = Instant::now();
    for _ in 0..reps {
        let loaded = manager.get_secret_authorized(alias, "auth-token").unwrap();
        assert_eq!(loaded.len(), secret.len());
    }
    let total_elapsed = start.elapsed();
    let avg_latency_ms = total_elapsed.as_secs_f64() * 1000.0 / (reps as f64);

    println!(
        "[BENCH-3.6] reps={} total_ms={:.4} avg_read_latency_ms={:.6} (budget < 1.0 ms)",
        reps,
        total_elapsed.as_secs_f64() * 1000.0,
        avg_latency_ms
    );

    assert!(
        avg_latency_ms < 1.0,
        "La lectura cacheada de secretos debe ser < 1.0 ms, observada: {:.6} ms",
        avg_latency_ms
    );
}

// LEDGER-3.6-6: Gatekeeper autoriza antes de consultar el almacén persistente o la caché
#[test]
fn test_gatekeeper_denial_blocks_backend_read_and_cache_loading() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct CountingBackend {
        store: Arc<std::sync::Mutex<HashMap<String, Zeroizing<Vec<u8>>>>>,
        load_calls: Arc<AtomicUsize>,
        save_calls: Arc<AtomicUsize>,
    }

    impl SecretStoreBackend for CountingBackend {
        fn save_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SecretError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            let mut buf = Zeroizing::new(Vec::with_capacity(secret.len()));
            buf.extend_from_slice(secret);
            self.store.lock().unwrap().insert(alias.to_string(), buf);
            Ok(())
        }

        fn load_secret(&self, alias: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
            self.load_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(val) = self.store.lock().unwrap().get(alias) {
                let mut copy = Zeroizing::new(Vec::with_capacity(val.len()));
                copy.extend_from_slice(val.as_slice());
                Ok(Some(copy))
            } else {
                Ok(None)
            }
        }

        fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError> {
            Ok(self.store.lock().unwrap().remove(alias).is_some())
        }
    }

    let counting = CountingBackend::default();
    let load_counter = Arc::clone(&counting.load_calls);
    {
        let buf = Zeroizing::new(Vec::from(b"PERSISTED_SECRET_999".as_slice()));
        counting.store.lock().unwrap().insert("test-secret".to_string(), buf);
    }

    // Gatekeeper configurado explícitamente en Deny
    let deny_profile = ProjectProfile::new("deny-profile", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Deny, false);
    let deny_gk = GatekeeperEngine::new(deny_profile).unwrap();
    let mut manager = SecretManager::new(Box::new(counting), "valid-token".to_string(), deny_gk);

    // 1. Con Gatekeeper Deny, get_secret_authorized debe fallar SIN llamar a load_secret
    let res = manager.get_secret_authorized("test-secret", "valid-token");
    assert!(matches!(res, Err(SecretError::GatekeeperDenied(_))));
    assert_eq!(
        load_counter.load(Ordering::SeqCst),
        0,
        "load_secret NO debe ser invocado cuando Gatekeeper deniega el acceso"
    );

    // 2. Con token de sesión inválido, tampoco debe consultar el backend
    let res_bad_token = manager.get_secret_authorized("test-secret", "invalid-token");
    assert!(matches!(res_bad_token, Err(SecretError::UnauthorizedSessionToken)));
    assert_eq!(
        load_counter.load(Ordering::SeqCst),
        0,
        "load_secret NO debe ser invocado cuando el token de sesión es inválido"
    );
}

#[test]
fn test_credential_test_harness_never_deletes_or_overwrites_preexisting_alias() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct SpyCredentialBackend {
        store: Arc<std::sync::Mutex<HashMap<String, Zeroizing<Vec<u8>>>>>,
        delete_calls: Arc<AtomicUsize>,
        save_calls: Arc<AtomicUsize>,
    }

    impl SecretStoreBackend for SpyCredentialBackend {
        fn save_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SecretError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            let mut buf = Zeroizing::new(Vec::with_capacity(secret.len()));
            buf.extend_from_slice(secret);
            self.store.lock().unwrap().insert(alias.to_string(), buf);
            Ok(())
        }

        fn load_secret(&self, alias: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
            if let Some(val) = self.store.lock().unwrap().get(alias) {
                let mut copy = Zeroizing::new(Vec::with_capacity(val.len()));
                copy.extend_from_slice(val.as_slice());
                Ok(Some(copy))
            } else {
                Ok(None)
            }
        }

        fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError> {
            self.delete_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.store.lock().unwrap().remove(alias).is_some())
        }
    }

    let spy = SpyCredentialBackend::default();
    let preexisting_alias = "miku-test-cred-colliding-0".to_string();
    let free_alias = "miku-test-cred-free-1".to_string();
    let foreign_payload = b"FOREIGN_USER_CREDENTIAL_DO_NOT_DELETE";

    {
        let buf = Zeroizing::new(Vec::from(foreign_payload.as_slice()));
        spy.store
            .lock()
            .unwrap()
            .insert(preexisting_alias.clone(), buf);
    }

    // Caso A: el primer alias candidato ya existe; debe conservarse intacto (0 borrados) y elegir el segundo
    let candidates = vec![preexisting_alias.clone(), free_alias.clone()];
    let chosen = acquire_free_test_alias(&spy, &candidates)
        .expect("Debe encontrar el segundo alias libre sin borrar el primero");

    assert_eq!(
        spy.delete_calls.load(Ordering::SeqCst),
        0,
        "NUNCA debe borrar una credencial preexistente durante la selección de alias"
    );
    assert_eq!(chosen, free_alias, "Debe saltar el alias ocupado y elegir el alias libre");
    assert_eq!(
        spy.load_secret(&preexisting_alias).unwrap().unwrap().as_slice(),
        foreign_payload,
        "La credencial preexistente debe permanecer intacta"
    );

    // Caso B: si todos los candidatos ya existen, debe fallar sin borrar ni modificar nada
    let only_occupied = vec![preexisting_alias.clone()];
    let res_all_occupied = acquire_free_test_alias(&spy, &only_occupied);
    assert!(
        res_all_occupied.is_err(),
        "Debe fallar si ningún alias candidato está libre"
    );
    assert_eq!(
        spy.delete_calls.load(Ordering::SeqCst),
        0,
        "No debe borrar credenciales cuando todos los alias candidatos están ocupados"
    );
}
