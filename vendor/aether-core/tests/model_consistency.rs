use aether_core::{Vault, VaultError};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn create_tmp_vault() -> Result<(TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let path = dir.path().join("test.mv2");
    // Create new memory
    {
        let mut mem = Vault::create(&path)?;
        mem.commit()?;
    }
    Ok((dir, path))
}

fn open_tmp_vault(path: &Path) -> Result<Vault, Box<dyn std::error::Error>> {
    Ok(Vault::open(path)?)
}

#[test]
fn test_vec_model_consistency() -> TestResult {
    let (_dir, path) = create_tmp_vault()?;

    // 1. Create index and set model "A"
    {
        let mut vault = open_tmp_vault(&path)?;
        aethervault.enable_vec()?;
        aethervault.set_vec_model("model-a")?;
        aethervault.aimit()?;
    }

    // 2. Open and verify "model-a" matches and "model-b" fails
    {
        let mut vault = open_tmp_vault(&path)?;
        // Should succeed (idempotent)
        aethervault.set_vec_model("model-a")?;

        // Should fail
        let result = aethervault.set_vec_model("model-b");
        assert!(result.is_err());
        match result {
            Err(VaultError::ModelMismatch { expected, actual }) => {
                assert_eq!(expected, "model-a");
                assert_eq!(actual, "model-b");
            }
            _ => panic!("Expected ModelMismatch error"),
        }
    }

    Ok(())
}

#[test]
fn test_vec_model_persistence() -> TestResult {
    let (_dir, path) = create_tmp_vault()?;

    // 1. Create index with model
    {
        let mut vault = open_tmp_vault(&path)?;
        aethervault.enable_vec()?;
        aethervault.set_vec_model("persistent-model")?;
        aethervault.aimit()?;
    }

    // 2. Open and check if model is loaded automatically
    {
        let mut vault = open_tmp_vault(&path)?;
        // We can inspect internal state via debug or by trying to set a mismatch
        let result = aethervault.set_vec_model("wrong-model");
        assert!(result.is_err());

        // Verify set_vec_model("persistent-model") works (confirming loaded state)
        aethervault.set_vec_model("persistent-model")?;
    }

    Ok(())
}
