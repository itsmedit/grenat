//! Authenticating on an SSH connection whose host key is already trusted:
//! with a private key (signing; the key never leaves this process) or a
//! password.

use std::sync::Arc;

use russh::client::Handle;
use russh::keys::PrivateKeyWithHashAlg;
use russh::keys::ssh_key::{Algorithm, HashAlg, PrivateKey};

use crate::error::{Error, ErrorKind};
use crate::handler::Client;

/// A credential ready to be offered: a decoded key, or a password.
pub enum Credential<'a> {
    Key(Box<PrivateKey>),
    Password(&'a str),
}

/// Offers `credential` for `user`; an error if the server refuses it.
/// `target` names the server in messages.
pub async fn authenticate(
    handle: &mut Handle<Client>,
    user: &str,
    credential: Credential<'_>,
    target: &str,
) -> Result<(), Error> {
    let (result, refused) = match credential {
        Credential::Password(password) => {
            (handle.authenticate_password(user, password).await?, format!("the password of `{user}`"))
        }
        Credential::Key(key) => {
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256);
            let hash = match key.algorithm() {
                Algorithm::Rsa { .. } => handle.best_supported_rsa_hash().await?.flatten(),
                _ => None,
            };
            let signer = PrivateKeyWithHashAlg::new(Arc::new(*key), hash);
            (handle.authenticate_publickey(user, signer).await?, format!("the key {fingerprint} for `{user}`"))
        }
    };
    if result.success() { Ok(()) } else { Err(Error::new(ErrorKind::Auth, format!("{target} refused {refused}"))) }
}
