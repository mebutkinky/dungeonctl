test:
    cargo test --workspace --all-features

update-readme:
    #!/usr/bin/env bash
    status=0
    cargo rdme --intralinks-all-features || status=$?
    cargo rdme --intralinks-all-features --manifest-path dungeonctl-audio/Cargo.toml || status=$?
    cargo rdme --intralinks-all-features --manifest-path dungeonctl-core/Cargo.toml || status=$?
    cargo rdme --intralinks-all-features --manifest-path dungeonctl-device/Cargo.toml || status=$?
    cargo rdme --intralinks-all-features --manifest-path dungeonctl-pattern/Cargo.toml || status=$?
    cargo rdme --intralinks-all-features --manifest-path dungeonctl-utils/Cargo.toml || status=$?
    exit "$status"

check-readme:
    #!/usr/bin/env bash
    status=0
    cargo rdme --check --intralinks-all-features || status=$?
    cargo rdme --check --intralinks-all-features --manifest-path dungeonctl-audio/Cargo.toml || status=$?
    cargo rdme --check --intralinks-all-features --manifest-path dungeonctl-core/Cargo.toml || status=$?
    cargo rdme --check --intralinks-all-features --manifest-path dungeonctl-device/Cargo.toml || status=$?
    cargo rdme --check --intralinks-all-features --manifest-path dungeonctl-pattern/Cargo.toml || status=$?
    cargo rdme --check --intralinks-all-features --manifest-path dungeonctl-utils/Cargo.toml || status=$?
    exit "$status"
