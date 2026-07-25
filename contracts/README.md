# Erebus contracts

Cairo. Poulav owns this directory.

`probes/` holds throwaway conformance probes against `starkware-libs/starknet-privacy`.
They are not part of the Scarb package: the upstream test harness is `#[cfg(test)]`-gated
inside `packages/privacy`, so a probe has to be copied into that checkout to run.
See the header of each probe file.
