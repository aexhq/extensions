//! Bounded HTTP object transfer against one-purpose storage authorities.

use crate::*;

pub(crate) async fn fetch_bundle(
    http: &reqwest::Client,
    fetch: &BundleFetch,
) -> EnvironmentResult<Vec<u8>> {
    if fetch.expires_at_ms.get() <= now_ms()
        || fetch.max_bytes.get() as usize > environment_wire::MAX_TOOL_BUNDLE_BYTES
    {
        return Err(invalid(
            "bundle fetch authority is expired or exceeds the bundle bound",
        ));
    }
    let response = authorized_get(
        http,
        fetch.url.as_str(),
        fetch
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        fetch.expires_at_ms.get(),
    )
    .await?;
    let staged = stage_response(response, fetch.max_bytes.get(), fetch.expires_at_ms.get()).await?;
    let bytes = tokio::fs::read(staged.file.path())
        .await
        .map_err(|error| temporary_from("verified bundle staging is unavailable", error))?;
    if hex::encode(Sha256::digest(&bytes)) != fetch.bundle_digest.as_str() {
        return Err(invalid("fetched bundle does not match its digest"));
    }
    Ok(bytes)
}

pub(crate) async fn fetch_object(
    http: &reqwest::Client,
    authority: &ObjectTransferAuthority,
    object: &ObjectReference,
) -> EnvironmentResult<StagedObject> {
    if authority.object_id != object.object_id {
        return Err(invalid(
            "object fetch authority is sealed to a different object identity",
        ));
    }
    validate_transfer_authority(authority, ObjectTransferAuthorityMethod::Get, object.bytes)?;
    let response = authorized_get(
        http,
        authority.url.as_str(),
        authority
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        authority.expires_at_ms.get(),
    )
    .await?;
    let staged = stage_response(response, object.bytes, authority.expires_at_ms.get()).await?;
    if staged.bytes != object.bytes || staged.sha256 != object.sha256.as_str() {
        return Err(invalid(
            "downloaded object does not match its immutable reference",
        ));
    }
    Ok(staged)
}

pub(crate) async fn authorized_get<'a>(
    http: &reqwest::Client,
    url: &str,
    headers: impl Iterator<Item = (&'a str, &'a str)>,
    expires_at_ms: u64,
) -> EnvironmentResult<reqwest::Response> {
    let url = validate_https_authority_url(url)?;
    let request = apply_authority_headers(http.get(url), headers)?;
    let timeout = transfer_timeout(expires_at_ms)?;
    let response = request
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| temporary_from("authorized object download failed", error))?;
    if !response.status().is_success() {
        return Err(temporary("authorized object download was refused"));
    }
    Ok(response)
}

pub(crate) async fn stage_response(
    response: reqwest::Response,
    limit: u64,
    expires_at_ms: u64,
) -> EnvironmentResult<StagedObject> {
    if limit > MAX_OBJECT_BYTES {
        return Err(invalid(
            "authorized object exceeds the 512 MiB transfer bound",
        ));
    }
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err(error(
            EnvironmentErrorCode::ResourceExhausted,
            false,
            "authorized object exceeds its byte bound",
        ));
    }
    let file = tempfile::NamedTempFile::new()
        .map_err(|error| temporary_from("supervisor object staging is unavailable", error))?;
    let std_file = file
        .reopen()
        .map_err(|error| temporary_from("supervisor object staging is unavailable", error))?;
    let mut output = tokio::fs::File::from_std(std_file);
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    let mut stream = response.bytes_stream();
    loop {
        // `Response::bytes_stream` may remain pending without yielding another chunk. Bound that
        // wait itself by the one-purpose authority deadline; checking only after a chunk arrives
        // would let a stalled guest export hold supervisor resources after its grant expired.
        let wait = transfer_timeout(expires_at_ms)?;
        let next = tokio::time::timeout(wait, stream.next())
            .await
            .map_err(|_| {
                if now_ms() >= expires_at_ms {
                    invalid("object transfer authority expired during download")
                } else {
                    temporary("authorized object stream exceeded its bounded transfer wait")
                }
            })?;
        let Some(chunk) = next else {
            break;
        };
        let chunk =
            chunk.map_err(|error| temporary_from("authorized object stream failed", error))?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > limit {
            return Err(error(
                EnvironmentErrorCode::ResourceExhausted,
                false,
                "authorized object exceeds its byte bound",
            ));
        }
        hash.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut output, &chunk)
            .await
            .map_err(|error| temporary_from("supervisor object staging failed", error))?;
    }
    tokio::io::AsyncWriteExt::flush(&mut output)
        .await
        .map_err(|error| temporary_from("supervisor object staging failed", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| temporary_from("supervisor object staging sync failed", error))?;
    drop(output);
    Ok(StagedObject {
        file,
        bytes,
        sha256: hex::encode(hash.finalize()),
    })
}

pub(crate) async fn put_object(
    http: &reqwest::Client,
    authority: &ObjectTransferAuthority,
    staged: &StagedObject,
) -> EnvironmentResult<()> {
    validate_transfer_authority(authority, ObjectTransferAuthorityMethod::Put, staged.bytes)?;
    let url = validate_https_authority_url(authority.url.as_str())?;
    let file = tokio::fs::File::open(staged.file.path())
        .await
        .map_err(|error| temporary_from("supervisor object staging is unavailable", error))?;
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(
        tokio::io::AsyncReadExt::take(file, staged.bytes.saturating_add(1)),
    ));
    let request = apply_authority_headers(
        http.put(url),
        authority
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    )?;
    let response = request
        .header(reqwest::header::CONTENT_LENGTH, staged.bytes)
        .body(body)
        .timeout(transfer_timeout(authority.expires_at_ms.get())?)
        .send()
        .await
        .map_err(|error| temporary_from("authorized object upload failed", error))?;
    if response.status().is_success() && now_ms() < authority.expires_at_ms.get() {
        Ok(())
    } else {
        Err(temporary("authorized object upload was refused"))
    }
}

pub(crate) fn validate_transfer_authority(
    authority: &ObjectTransferAuthority,
    method: ObjectTransferAuthorityMethod,
    required_bytes: u64,
) -> EnvironmentResult<()> {
    if authority.method != method
        || authority.expires_at_ms.get() <= now_ms()
        || authority.max_bytes.get() < required_bytes
        || authority.max_bytes.get() > MAX_OBJECT_BYTES
        || required_bytes > MAX_OBJECT_BYTES
    {
        return Err(invalid(
            "object authority does not cover the bounded transfer",
        ));
    }
    validate_https_authority_url(authority.url.as_str())?;
    Ok(())
}

pub(crate) fn validate_https_authority_url(value: &str) -> EnvironmentResult<reqwest::Url> {
    let url =
        reqwest::Url::parse(value).map_err(|_| invalid("transfer authority URL is invalid"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "transfer authority must be a sealed HTTPS URL without credentials",
        ));
    }
    Ok(url)
}

pub(crate) fn apply_authority_headers<'a>(
    mut request: reqwest::RequestBuilder,
    headers: impl Iterator<Item = (&'a str, &'a str)>,
) -> EnvironmentResult<reqwest::RequestBuilder> {
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| invalid("transfer authority header name is invalid"))?;
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "proxy-connection"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        ) {
            return Err(invalid(
                "transfer authority contains a forbidden transport header",
            ));
        }
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| invalid("transfer authority header value is invalid"))?;
        request = request.header(name, value);
    }
    Ok(request)
}

pub(crate) fn transfer_timeout(expires_at_ms: u64) -> EnvironmentResult<Duration> {
    let remaining = expires_at_ms.saturating_sub(now_ms());
    if remaining == 0 {
        return Err(invalid("transfer authority is expired"));
    }
    Ok(Duration::from_millis(remaining.min(15 * 60 * 1_000)))
}
