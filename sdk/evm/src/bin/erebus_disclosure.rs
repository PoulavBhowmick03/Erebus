//! Local disclosure operations with public-bound payment verification.

#[tokio::main]
async fn main() {
    erebus_evm::disclosure::cli::run(erebus_evm::disclosure::cli::verify_public_payment).await;
}
