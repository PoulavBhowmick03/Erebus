//! Local disclosure operations with public-bound and x402 exact payment verification.

#[tokio::main]
async fn main() {
    erebus_evm::disclosure::cli::run(|request| async move {
        if erebus_evm::disclosure::cli::is_x402_request(&request.deployment) {
            return erebus_evm::disclosure::cli::verify_x402_payment(request).await;
        }
        erebus_evm::disclosure::cli::verify_public_payment(request).await
    })
    .await;
}
