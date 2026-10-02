//! Runs only after `cd circuits/m5 && npm ci && npm run prototype`.

use std::{fs, path::PathBuf};

use erebus_shielded_prover::{ProverError, ProvingArtifacts};
use serde_json::Value;

fn artifacts(name: &str) -> (ProvingArtifacts, Value) {
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build");
    let manifest: Value = serde_json::from_slice(
        &fs::read(build.join("artifact-manifest.json")).expect("M5 local artifacts"),
    )
    .expect("manifest JSON");
    let entry = &manifest["circuits"][name];
    let string = |field: &str| entry[field].as_str().expect("manifest hash").to_owned();
    let artifacts = ProvingArtifacts {
        wasm: build.join(format!("{name}_js/{name}.wasm")),
        r1cs: build.join(format!("{name}.r1cs")),
        zkey: build.join(format!("{name}.zkey")),
        wasm_sha256: string("wasmSha256"),
        r1cs_sha256: string("r1csSha256"),
        zkey_sha256: string("zkeySha256"),
    };
    let input: Value = serde_json::from_slice(
        &fs::read(build.join(format!("{name}-input.json"))).expect("M5 witness input"),
    )
    .expect("input JSON");
    (artifacts, input)
}

#[test]
#[ignore = "requires M5 local artifact generation"]
fn native_rust_proves_the_same_pool_transitions() {
    let transitions: &[(&str, &[&str], usize)] = &[
        (
            "deposit",
            &[
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "amount",
                "noteCommitment",
            ],
            4,
        ),
        (
            "transfer",
            &[
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "dealCommitment",
                "dealNullifier",
                "root",
                "inputNullifier",
                "paymentCommitment",
                "changeCommitment",
                "expiry",
            ],
            8,
        ),
        (
            "withdraw",
            &[
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "root",
                "noteNullifier",
                "recipient",
                "amount",
            ],
            6,
        ),
    ];
    for (name, public_names, changed_index) in transitions {
        let (artifacts, input) = artifacts(name);
        let expected: Vec<&str> = public_names
            .iter()
            .map(|field| input[*field].as_str().expect("public input"))
            .collect();
        let mut altered = artifacts.clone();
        altered.wasm_sha256 = "0".repeat(64);
        assert!(matches!(
            altered.verify(),
            Err(ProverError::Artifact("wasm"))
        ));
        let proof = artifacts
            .prove(&input, &expected)
            .expect("native local proof");
        assert_eq!(proof.public_inputs.len(), expected.len());
        if *name == "transfer" {
            let output = std::env::var_os("EREBUS_M5_TEST_OUTPUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build")
                })
                .join("rust-transfer-calldata.json");
            fs::write(
                output,
                serde_json::to_vec(&proof.solidity_calldata()).expect("proof JSON"),
            )
            .expect("write public proof calldata");
        }
        let mut wrong = expected;
        wrong[*changed_index] = "1";
        assert!(matches!(
            artifacts.prove(&input, &wrong),
            Err(ProverError::PublicInputMismatch)
        ));
    }
}
