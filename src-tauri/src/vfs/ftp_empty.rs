//! OpenDAL's FTP writer only opens a data channel on the first non-empty write, so a
//! zero-byte upload never creates the file. This issues a plain `STOR` with no data
//! for that case (used by "New file" and by copies of empty files).

use crate::error::{AppError, AppResult};
use std::sync::Arc;
use suppaftp::types::FileType;
use suppaftp::{AsyncRustlsConnector, AsyncRustlsFtpStream, FtpError, ImplAsyncFtpStream};

#[derive(Clone)]
pub struct FtpTarget {
    /// `host:port`
    pub addr: String,
    pub secure: bool,
    pub user: String,
    pub password: String,
    /// Normalized root (`/` or `/a/b`).
    pub root: String,
}

fn err(e: FtpError) -> AppError {
    AppError::Connection(e.to_string())
}

pub async fn create_empty(t: &FtpTarget, path: &str) -> AppResult<()> {
    let stream = AsyncRustlsFtpStream::connect(&t.addr).await.map_err(err)?;
    let mut ftp: ImplAsyncFtpStream<_> = if t.secure {
        let mut roots = suppaftp::rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        let cfg = suppaftp::rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let host = t.addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&t.addr).to_string();
        stream.into_secure(AsyncRustlsConnector::from(futures_rustls::TlsConnector::from(Arc::new(cfg))), &host).await.map_err(err)?
    } else {
        stream
    };
    if !t.user.is_empty() {
        ftp.login(&t.user, &t.password).await.map_err(|e| AppError::Auth(e.to_string()))?;
    }
    ftp.transfer_type(FileType::Binary).await.map_err(err)?;
    let full = if t.root == "/" { path.to_string() } else { format!("{}{}", t.root.trim_end_matches('/'), path) };
    let mut empty: &[u8] = &[];
    ftp.put_file(&full, &mut empty).await.map_err(err)?;
    let _ = ftp.quit().await;
    Ok(())
}
