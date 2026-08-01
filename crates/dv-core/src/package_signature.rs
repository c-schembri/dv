use std::{
  fs,
  io::{self, Read, Seek, SeekFrom},
  mem::{align_of, size_of},
  path::{Path, PathBuf},
  sync::{Arc, OnceLock},
  time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use cms::{
  cert::CertificateChoices,
  content_info::ContentInfo,
  signed_data::{SignedAttributes, SignedData, SignerIdentifier, SignerInfo},
};
use der::{
  Any, Decode, Encode, Reader, Sequence,
  asn1::{GeneralizedTime, Ia5String, ObjectIdentifier, OctetString},
};
#[cfg(not(windows))]
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
use webpki::{ALL_VERIFICATION_ALGS, EndEntityCert, KeyUsage, anchor_from_trusted_cert};
use x509_cert::{
  Certificate,
  ext::pkix::{SubjectKeyIdentifier, name::GeneralName},
  serial_number::SerialNumber,
  spki,
};

use super::{BENCHMARK_CACHE_LINE_BYTES, PackageError, PackageErrorKind, SignatureValidationMode};

const SIGNATURE_PATH: &[u8] = b".signature.p7s";
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024 * 1024;
const CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const CENTRAL_DIRECTORY_FIXED_BYTES: u64 = 46;
const LOCAL_FILE_FIXED_BYTES: u64 = 30;
const CODE_SIGNING_EKU: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];
const TIME_STAMPING_EKU: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x08];

const OID_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const OID_SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const OID_CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const OID_MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const OID_COUNTERSIGNATURE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.6");
const OID_TIMESTAMP_TOKEN: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");
const OID_SIGNING_CERTIFICATE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.12");
const OID_SIGNING_CERTIFICATE_V2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const OID_TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
const OID_COMMITMENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.16");
const OID_AUTHOR: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.6.1");
const OID_REPOSITORY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.6.2");
const OID_REPOSITORY_URL: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.84.2.1.1.1");
const OID_PACKAGE_OWNERS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.311.84.2.1.1.2");
const OID_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const OID_SHA256_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const OID_SHA384_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.12");
const OID_SHA512_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.13");
const OID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const OID_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.2");
const OID_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.3");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TrustedSignerKind {
  Author,
  Repository,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FingerprintAlgorithm {
  Sha256,
  Sha384,
  Sha512,
}

impl FingerprintAlgorithm {
  pub(super) fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("SHA256") {
      Some(Self::Sha256)
    } else if value.eq_ignore_ascii_case("SHA384") {
      Some(Self::Sha384)
    } else if value.eq_ignore_ascii_case("SHA512") {
      Some(Self::Sha512)
    } else {
      None
    }
  }

  const fn bytes(self) -> usize {
    match self {
      Self::Sha256 => 32,
      Self::Sha384 => 48,
      Self::Sha512 => 64,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TrustedCertificate {
  pub(super) fingerprint: Box<[u8]>,
  pub(super) algorithm: FingerprintAlgorithm,
  pub(super) allow_untrusted_root: bool,
}

impl TrustedCertificate {
  pub(super) fn parse(fingerprint: &str, algorithm: FingerprintAlgorithm, allow_untrusted_root: bool) -> Result<Self, &'static str> {
    if fingerprint.len() != algorithm.bytes() * 2 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
      return Err("certificate fingerprint has the wrong length or contains non-hexadecimal characters");
    }
    let mut decoded = Vec::with_capacity(algorithm.bytes());
    for pair in fingerprint.as_bytes().chunks_exact(2) {
      decoded.push((hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]));
    }
    Ok(Self {
      fingerprint: decoded.into_boxed_slice(),
      algorithm,
      allow_untrusted_root,
    })
  }
}

const fn hex_nibble(value: u8) -> u8 {
  match value {
    b'0'..=b'9' => value - b'0',
    b'a'..=b'f' => value - b'a' + 10,
    b'A'..=b'F' => value - b'A' + 10,
    _ => 0,
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TrustedSigner {
  pub(super) name: String,
  pub(super) service_index: Option<String>,
  pub(super) owners: Box<[String]>,
  pub(super) certificates: Box<[TrustedCertificate]>,
  pub(super) kind: TrustedSignerKind,
}

pub(super) struct SignaturePolicy {
  pub(super) signers: Arc<[TrustedSigner]>,
  pub(super) mode: SignatureValidationMode,
  #[cfg(not(windows))]
  sdk_root: Option<PathBuf>,
  #[cfg(not(windows))]
  timestamp_roots: OnceLock<Result<Box<[TrustAnchor<'static>]>, String>>,
  #[cfg(not(windows))]
  code_signing_roots: OnceLock<Result<Box<[TrustAnchor<'static>]>, String>>,
}

impl SignaturePolicy {
  pub(super) fn new(mode: SignatureValidationMode, signers: Vec<TrustedSigner>) -> Self {
    Self {
      signers: signers.into(),
      mode,
      #[cfg(not(windows))]
      sdk_root: None,
      #[cfg(not(windows))]
      timestamp_roots: OnceLock::new(),
      #[cfg(not(windows))]
      code_signing_roots: OnceLock::new(),
    }
  }

  pub(super) fn set_sdk_root(&mut self, sdk_root: PathBuf) {
    #[cfg(not(windows))]
    {
      self.sdk_root = Some(sdk_root);
    }
    #[cfg(windows)]
    let _ = sdk_root;
  }

  pub(super) fn validate(&self) -> Result<(), PackageError> {
    if self.mode == SignatureValidationMode::Require && self.signers.is_empty() {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        "trustedSigners",
        "signatureValidationMode=require needs at least one author or repository in trustedSigners",
      ));
    }
    Ok(())
  }

  #[cfg(windows)]
  fn timestamp_trust_anchors(&self) -> Result<&[TrustAnchor<'static>], PackageError> {
    platform_trust_anchors()
  }

  #[cfg(not(windows))]
  fn timestamp_trust_anchors(&self) -> Result<&[TrustAnchor<'static>], PackageError> {
    self.sdk_trust_anchors(&self.timestamp_roots, "timestampctl.pem", "timestamp")
  }

  #[cfg(windows)]
  fn code_signing_trust_anchors(&self) -> Result<&[TrustAnchor<'static>], PackageError> {
    platform_trust_anchors()
  }

  #[cfg(not(windows))]
  fn code_signing_trust_anchors(&self) -> Result<&[TrustAnchor<'static>], PackageError> {
    const SYSTEM_OBJECT_ROOTS: &str = "/etc/pki/ca-trust/extracted/pem/objsign-ca-bundle.pem";
    match self.code_signing_roots.get_or_init(|| {
      let system = Path::new(SYSTEM_OBJECT_ROOTS);
      if system.is_file() {
        load_pem_anchors(system)
      } else {
        self.load_sdk_trust_anchors("codesignctl.pem")
      }
    }) {
      Ok(anchors) => Ok(anchors),
      Err(error) => Err(signature_error("code-signing certificate roots", error)),
    }
  }

  #[cfg(not(windows))]
  fn sdk_trust_anchors<'a>(
    &'a self,
    cache: &'a OnceLock<Result<Box<[TrustAnchor<'static>]>, String>>,
    file_name: &str,
    purpose: &str,
  ) -> Result<&'a [TrustAnchor<'static>], PackageError> {
    match cache.get_or_init(|| self.load_sdk_trust_anchors(file_name)) {
      Ok(anchors) => Ok(anchors),
      Err(error) => Err(signature_error(format!("{purpose} certificate roots"), error)),
    }
  }

  #[cfg(not(windows))]
  fn load_sdk_trust_anchors(&self, file_name: &str) -> Result<Box<[TrustAnchor<'static>]>, String> {
    let sdk_root = self.sdk_root.as_ref().ok_or_else(|| "the selected .NET SDK path is unavailable".to_owned())?;
    load_pem_anchors(&sdk_root.join("trustedroots").join(file_name))
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignatureKind {
  Author,
  Repository,
}

#[derive(Clone)]
struct CentralDirectoryRecord {
  position: u64,
  local_offset: u64,
  file_entry_size: u64,
  header_size: u64,
  change_in_offset: i64,
  flags: u16,
  compression: u16,
  compressed_size: u32,
  uncompressed_size: u32,
  external_attributes: u32,
}

const _: () = assert!(size_of::<CentralDirectoryRecord>() == 56);
const _: () = assert!(align_of::<CentralDirectoryRecord>() == 8);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<CentralDirectoryRecord>() == 1);

struct SignedArchive {
  records: Vec<CentralDirectoryRecord>,
  signature_index: usize,
  start_of_local_headers: u64,
  eocd: u64,
  file_len: u64,
}

const _: () = assert!(size_of::<SignedArchive>() == 56);
const _: () = assert!(align_of::<SignedArchive>() == 8);

struct ParsedCms {
  signed_data: SignedData,
  certificate_der: Vec<Vec<u8>>,
}

pub(super) fn verify_package(path: &Path, policy: &SignaturePolicy) -> Result<bool, PackageError> {
  let mut file = fs::File::open(path).map_err(|error| signature_io(path, "open signed package", error))?;
  let archive = match SignedArchive::read(&mut file, path)? {
    Some(archive) => archive,
    None if policy.mode == SignatureValidationMode::Accept => return Ok(false),
    None => return Err(signature_error(path, "package is unsigned but signatureValidationMode=require")),
  };
  let signature = archive.read_signature(&mut file, path)?;
  let parsed = parse_cms(&signature, path)?;
  verify_primary(path, &mut file, &archive, &parsed, policy)?;
  Ok(true)
}

fn parse_cms(bytes: &[u8], path: &Path) -> Result<ParsedCms, PackageError> {
  let content = ContentInfo::from_der(bytes).map_err(|error| signature_error(path, format!("invalid package CMS content: {error}")))?;
  if content.content_type != OID_SIGNED_DATA {
    return Err(signature_error(path, "package signature CMS content type is not signed-data"));
  }
  let signed_data: SignedData = content
    .content
    .decode_as()
    .map_err(|error| signature_error(path, format!("invalid package CMS signed-data: {error}")))?;
  let certificate_der = collect_certificate_der(&signed_data, path, "package signature")?;
  Ok(ParsedCms { signed_data, certificate_der })
}

fn collect_certificate_der(signed_data: &SignedData, path: &Path, context: &str) -> Result<Vec<Vec<u8>>, PackageError> {
  signed_data
    .certificates
    .as_ref()
    .map(|certificates| {
      certificates
        .0
        .iter()
        .map(|choice| match choice {
          CertificateChoices::Certificate(certificate) => certificate
            .to_der()
            .map_err(|error| signature_error(path, format!("could not encode {context} certificate: {error}"))),
          CertificateChoices::Other(_) => Err(signature_error(path, format!("{context} contains a non-X.509 certificate"))),
        })
        .collect()
    })
    .unwrap_or_else(|| Ok(Vec::new()))
}

fn verify_primary(path: &Path, file: &mut fs::File, archive: &SignedArchive, parsed: &ParsedCms, policy: &SignaturePolicy) -> Result<(), PackageError> {
  if parsed.signed_data.signer_infos.0.len() != 1 {
    return Err(signature_error(path, "package signature must contain exactly one primary signer"));
  }
  if parsed.signed_data.encap_content_info.econtent_type != OID_DATA {
    return Err(signature_error(path, "package signature encapsulated content type is not data"));
  }
  let signer = parsed.signed_data.signer_infos.0.get(0).expect("the primary signer count was checked");
  let content = parsed
    .signed_data
    .encap_content_info
    .econtent
    .as_ref()
    .ok_or_else(|| signature_error(path, "package signature has no encapsulated content"))?
    .value();
  verify_cms_signer(path, signer, content, parsed, Some(OID_DATA))?;
  let (hash_algorithm, expected_hash) = signature_content(content, path)?;
  let actual_hash = archive.unsigned_hash(file, hash_algorithm, path)?;
  if actual_hash != expected_hash {
    return Err(signature_error(path, "signed package content hash does not match the archive"));
  }
  let kind = signature_kind(signer.signed_attrs.as_ref(), path)?;
  let certificate_index = signer_certificate_index(signer, &parsed.signed_data, path)?;
  let primary_certificate = certificate(&parsed.signed_data, certificate_index);
  let owners = if kind == SignatureKind::Repository {
    repository_attributes(signer.signed_attrs.as_ref(), path)?.1
  } else {
    Vec::new()
  };
  if policy.mode == SignatureValidationMode::Accept {
    let _ = verify_timestamp(path, signer, None)?;
    if let Some(counter) = repository_countersignature(signer, path)? {
      verify_cms_signer(path, &counter, signer.signature.as_bytes(), parsed, None)?;
      let _ = repository_attributes(counter.signed_attrs.as_ref(), path)?;
      let _ = verify_timestamp(path, &counter, None)?;
    }
    return Ok(());
  }
  let primary_validation_time = verify_timestamp(path, signer, Some(policy))?;
  if let Some(allow_untrusted_root) = match_trusted(policy, kind, &parsed.certificate_der[certificate_index], &owners) {
    let validation_time = primary_validation_time.unwrap_or_else(SystemTime::now);
    return verify_certificate_chain(
      path,
      CertificateChainVerification {
        certificate_index,
        certificate: primary_certificate,
        certificate_der: &parsed.certificate_der,
        time: validation_time,
        eku: CODE_SIGNING_EKU,
        allow_untrusted_root,
        configured_anchors: policy.code_signing_trust_anchors()?,
      },
    );
  }
  if kind == SignatureKind::Repository {
    return Err(signature_error(path, "repository signature does not match trustedSigners"));
  }
  let counter = repository_countersignature(signer, path)?.ok_or_else(|| {
    signature_error(
      path,
      "author signature does not match trustedSigners and has no trusted repository countersignature",
    )
  })?;
  verify_cms_signer(path, &counter, signer.signature.as_bytes(), parsed, None)?;
  let counter_index = signer_certificate_index(&counter, &parsed.signed_data, path)?;
  let counter_certificate = certificate(&parsed.signed_data, counter_index);
  let (_, counter_owners) = repository_attributes(counter.signed_attrs.as_ref(), path)?;
  let allow_untrusted_root = match_trusted(policy, SignatureKind::Repository, &parsed.certificate_der[counter_index], &counter_owners)
    .ok_or_else(|| signature_error(path, "repository countersignature does not match trustedSigners"))?;
  let validation_time = verify_timestamp(path, &counter, Some(policy))?.unwrap_or_else(SystemTime::now);
  verify_certificate_chain(
    path,
    CertificateChainVerification {
      certificate_index: counter_index,
      certificate: counter_certificate,
      certificate_der: &parsed.certificate_der,
      time: validation_time,
      eku: CODE_SIGNING_EKU,
      allow_untrusted_root,
      configured_anchors: policy.code_signing_trust_anchors()?,
    },
  )
}

fn verify_cms_signer(
  path: &Path,
  signer: &SignerInfo,
  content: &[u8],
  parsed: &ParsedCms,
  expected_content_type: Option<ObjectIdentifier>,
) -> Result<(), PackageError> {
  let digest = digest(signer.digest_alg.oid, content, path)?;
  let attributes = signer
    .signed_attrs
    .as_ref()
    .ok_or_else(|| signature_error(path, "package signer has no signed attributes"))?;
  let certificate_index = signer_certificate_index(signer, &parsed.signed_data, path)?;
  let certificate_der = &parsed.certificate_der[certificate_index];
  validate_signed_attributes(
    path,
    attributes,
    &digest,
    expected_content_type,
    certificate_der,
    certificate(&parsed.signed_data, certificate_index),
  )?;
  let signed = attributes
    .to_der()
    .map_err(|error| signature_error(path, format!("could not encode package signed attributes: {error}")))?;
  let leaf_der = CertificateDer::from(certificate_der.as_slice());
  let leaf = EndEntityCert::try_from(&leaf_der).map_err(|error| signature_error(path, format!("invalid package signer certificate: {error}")))?;
  let algorithm = signature_algorithm(signer, path)?;
  leaf
    .verify_signature(algorithm, &signed, signer.signature.as_bytes())
    .map_err(|error| signature_error(path, format!("package CMS signature verification failed: {error}")))
}

fn validate_signed_attributes(
  path: &Path,
  attributes: &SignedAttributes,
  digest: &[u8],
  expected_content_type: Option<ObjectIdentifier>,
  certificate_der: &[u8],
  certificate: &Certificate,
) -> Result<(), PackageError> {
  let content_types = attribute_values(attributes, OID_CONTENT_TYPE);
  match expected_content_type {
    Some(expected) if content_types.len() == 1 && content_types[0].decode_as::<ObjectIdentifier>().ok() == Some(expected) => {},
    Some(_) => return Err(signature_error(path, "CMS signer contains an invalid content-type attribute")),
    None if content_types.is_empty() => {},
    None => return Err(signature_error(path, "CMS countersigner must not contain a content-type attribute")),
  }
  let digests = attribute_values(attributes, OID_MESSAGE_DIGEST);
  if digests.len() != 1 {
    return Err(signature_error(path, "package signer must contain one message-digest attribute"));
  }
  let actual = digests[0]
    .decode_as::<OctetString>()
    .map_err(|error| signature_error(path, format!("invalid package message-digest attribute: {error}")))?;
  if actual.as_bytes() != digest {
    return Err(signature_error(path, "package signer message digest does not match its content"));
  }
  validate_signing_certificate(path, attributes, certificate_der, certificate, expected_content_type == Some(OID_TST_INFO))?;
  Ok(())
}

#[derive(Sequence)]
struct SigningCertificateV2 {
  certificates: Vec<EssCertIdV2>,
  policies: Option<Any>,
}

#[derive(Sequence)]
struct EssCertIdV2 {
  hash_algorithm: Option<spki::AlgorithmIdentifierOwned>,
  certificate_hash: OctetString,
  issuer_serial: Option<IssuerSerial>,
}

#[derive(Sequence)]
struct SigningCertificateV1 {
  certificates: Vec<EssCertIdV1>,
  policies: Option<Any>,
}

#[derive(Sequence)]
struct EssCertIdV1 {
  certificate_hash: OctetString,
  issuer_serial: Option<IssuerSerial>,
}

#[derive(Sequence)]
struct IssuerSerial {
  issuer: Vec<GeneralName>,
  serial_number: SerialNumber,
}

fn validate_signing_certificate(
  path: &Path,
  attributes: &SignedAttributes,
  certificate_der: &[u8],
  certificate: &Certificate,
  timestamp: bool,
) -> Result<(), PackageError> {
  let v1 = unique_attribute_value(attributes, OID_SIGNING_CERTIFICATE, path, "signing-certificate")?;
  let v2 = unique_attribute_value(attributes, OID_SIGNING_CERTIFICATE_V2, path, "signing-certificate-v2")?;
  if timestamp {
    if v1.is_none() && v2.is_none() {
      return Err(signature_error(
        path,
        "timestamp signer has no signing-certificate or signing-certificate-v2 attribute",
      ));
    }
  } else if v1.is_some() || v2.is_none() {
    return Err(signature_error(
      path,
      "package signer requires signing-certificate-v2 and forbids signing-certificate",
    ));
  }

  if let Some(value) = v2 {
    let parsed = value
      .decode_as::<SigningCertificateV2>()
      .map_err(|error| signature_error(path, format!("invalid signing-certificate-v2 attribute: {error}")))?;
    let first = parsed
      .certificates
      .first()
      .ok_or_else(|| signature_error(path, "signing-certificate-v2 contains no certificates"))?;
    if parsed
      .certificates
      .iter()
      .any(|entry| !matches!(ess_v2_hash_oid(entry), OID_SHA256 | OID_SHA384 | OID_SHA512))
    {
      return Err(signature_error(path, "signing-certificate-v2 uses an unsupported certificate hash"));
    }
    if !ess_v2_matches(first, certificate_der, certificate, !timestamp) {
      return Err(signature_error(path, "signing-certificate-v2 does not identify the CMS signer certificate"));
    }
  }
  if let Some(value) = v1 {
    let parsed = value
      .decode_as::<SigningCertificateV1>()
      .map_err(|error| signature_error(path, format!("invalid signing-certificate attribute: {error}")))?;
    let first = parsed
      .certificates
      .first()
      .ok_or_else(|| signature_error(path, "signing-certificate contains no certificates"))?;
    if first.certificate_hash.as_bytes() != Sha1::digest(certificate_der).as_slice()
      || first.issuer_serial.as_ref().is_some_and(|issuer| !issuer_serial_matches(issuer, certificate))
    {
      return Err(signature_error(path, "signing-certificate does not identify the CMS signer certificate"));
    }
  }
  Ok(())
}

fn unique_attribute_value<'a>(attributes: &'a SignedAttributes, oid: ObjectIdentifier, path: &Path, name: &str) -> Result<Option<&'a Any>, PackageError> {
  let mut matches = attributes.iter().filter(|attribute| attribute.oid == oid);
  let Some(attribute) = matches.next() else {
    return Ok(None);
  };
  if matches.next().is_some() || attribute.values.len() != 1 {
    return Err(signature_error(
      path,
      format!("CMS signer must contain at most one {name} attribute with one value"),
    ));
  }
  Ok(attribute.values.iter().next())
}

fn ess_v2_matches(entry: &EssCertIdV2, certificate_der: &[u8], certificate: &Certificate, issuer_required: bool) -> bool {
  if issuer_required && entry.issuer_serial.is_none() {
    return false;
  }
  if entry.issuer_serial.as_ref().is_some_and(|issuer| !issuer_serial_matches(issuer, certificate)) {
    return false;
  }
  let fingerprint = match ess_v2_hash_oid(entry) {
    OID_SHA256 => Sha256::digest(certificate_der).to_vec(),
    OID_SHA384 => Sha384::digest(certificate_der).to_vec(),
    OID_SHA512 => Sha512::digest(certificate_der).to_vec(),
    _ => return false,
  };
  entry.certificate_hash.as_bytes() == fingerprint
}

fn ess_v2_hash_oid(entry: &EssCertIdV2) -> ObjectIdentifier {
  entry.hash_algorithm.as_ref().map_or(OID_SHA256, |algorithm| algorithm.oid)
}

fn issuer_serial_matches(issuer: &IssuerSerial, certificate: &Certificate) -> bool {
  issuer.serial_number == certificate.tbs_certificate.serial_number
    && issuer.issuer.len() == 1
    && matches!(&issuer.issuer[0], GeneralName::DirectoryName(name) if name == &certificate.tbs_certificate.issuer)
}

fn signature_algorithm(signer: &SignerInfo, path: &Path) -> Result<&'static dyn rustls_pki_types::SignatureVerificationAlgorithm, PackageError> {
  let oid = signer.signature_algorithm.oid;
  match signer.digest_alg.oid {
    OID_SHA256 if oid == OID_RSA || oid == OID_SHA256_RSA => Ok(webpki::ring::RSA_PKCS1_2048_8192_SHA256),
    OID_SHA384 if oid == OID_RSA || oid == OID_SHA384_RSA => Ok(webpki::ring::RSA_PKCS1_2048_8192_SHA384),
    OID_SHA512 if oid == OID_RSA || oid == OID_SHA512_RSA => Ok(webpki::ring::RSA_PKCS1_2048_8192_SHA512),
    _ => Err(signature_error(path, "package signatures require RSA with SHA-256, SHA-384, or SHA-512")),
  }
}

fn digest(oid: ObjectIdentifier, content: &[u8], path: &Path) -> Result<Vec<u8>, PackageError> {
  match oid {
    OID_SHA256 => Ok(Sha256::digest(content).to_vec()),
    OID_SHA384 => Ok(Sha384::digest(content).to_vec()),
    OID_SHA512 => Ok(Sha512::digest(content).to_vec()),
    _ => Err(signature_error(path, "package signature uses an unsupported digest algorithm")),
  }
}

fn signer_certificate_index(signer: &SignerInfo, signed_data: &SignedData, path: &Path) -> Result<usize, PackageError> {
  let certificates = signed_data
    .certificates
    .as_ref()
    .ok_or_else(|| signature_error(path, "package signature contains no certificates"))?;
  certificates
    .0
    .iter()
    .enumerate()
    .find_map(|(index, choice)| {
      let CertificateChoices::Certificate(certificate) = choice else {
        return None;
      };
      match &signer.sid {
        SignerIdentifier::IssuerAndSerialNumber(identifier)
          if identifier.issuer == certificate.tbs_certificate.issuer && identifier.serial_number == certificate.tbs_certificate.serial_number =>
        {
          Some(index)
        },
        SignerIdentifier::SubjectKeyIdentifier(identifier) => certificate
          .tbs_certificate
          .get::<SubjectKeyIdentifier>()
          .ok()
          .flatten()
          .filter(|(_, candidate)| candidate == identifier)
          .map(|_| index),
        _ => None,
      }
    })
    .ok_or_else(|| signature_error(path, "package signer certificate is missing"))
}

fn certificate(signed_data: &SignedData, index: usize) -> &Certificate {
  match signed_data.certificates.as_ref().expect("certificate set was checked").0.get(index) {
    Some(CertificateChoices::Certificate(certificate)) => certificate,
    _ => unreachable!("the signer certificate index refers to an X.509 certificate"),
  }
}

#[derive(Sequence)]
struct CommitmentType {
  commitment_type: ObjectIdentifier,
  qualifiers: Option<Any>,
}

fn signature_kind(attributes: Option<&SignedAttributes>, path: &Path) -> Result<SignatureKind, PackageError> {
  let attributes = attributes.ok_or_else(|| signature_error(path, "package signer has no signed attributes"))?;
  let value = unique_attribute_value(attributes, OID_COMMITMENT_TYPE, path, "commitment-type-indication")?
    .ok_or_else(|| signature_error(path, "package signer has no commitment-type-indication attribute"))?;
  let commitment = value
    .decode_as::<CommitmentType>()
    .map_err(|error| signature_error(path, format!("invalid commitment-type-indication: {error}")))?;
  if commitment.commitment_type == OID_AUTHOR {
    Ok(SignatureKind::Author)
  } else if commitment.commitment_type == OID_REPOSITORY {
    Ok(SignatureKind::Repository)
  } else {
    Err(signature_error(path, "package signer has an unsupported commitment type"))
  }
}

fn repository_attributes(attributes: Option<&SignedAttributes>, path: &Path) -> Result<(String, Vec<String>), PackageError> {
  let attributes = attributes.ok_or_else(|| signature_error(path, "repository signer has no signed attributes"))?;
  let urls = attribute_values(attributes, OID_REPOSITORY_URL);
  if urls.len() != 1 {
    return Err(signature_error(
      path,
      "repository signature must contain exactly one NuGet v3 service-index URL",
    ));
  }
  let url = urls[0]
    .decode_as::<Ia5String>()
    .map_err(|error| signature_error(path, format!("invalid repository service-index URL attribute: {error}")))?
    .to_string();
  let parsed_url = reqwest::Url::parse(&url).map_err(|error| signature_error(path, format!("invalid repository service-index URL attribute: {error}")))?;
  if parsed_url.scheme() != "https" || !parsed_url.has_host() {
    return Err(signature_error(path, "repository signature service-index URL must be absolute HTTPS"));
  }
  let values = attribute_values(attributes, OID_PACKAGE_OWNERS);
  if values.len() > 1 {
    return Err(signature_error(path, "repository signature contains more than one package-owners value"));
  }
  let owners = values
    .first()
    .map(|value| value.decode_as::<Vec<String>>())
    .transpose()
    .map_err(|error| signature_error(path, format!("invalid repository package-owners attribute: {error}")))?
    .unwrap_or_default();
  if !values.is_empty() && (owners.is_empty() || owners.iter().any(|owner| owner.trim().is_empty())) {
    return Err(signature_error(path, "repository package-owners attribute must contain non-empty owners"));
  }
  Ok((url, owners))
}

fn attribute_values(attributes: &SignedAttributes, oid: ObjectIdentifier) -> Vec<&Any> {
  attributes
    .iter()
    .filter(|attribute| attribute.oid == oid)
    .flat_map(|attribute| attribute.values.iter())
    .collect()
}

fn repository_countersignature(primary: &SignerInfo, path: &Path) -> Result<Option<SignerInfo>, PackageError> {
  let Some(attributes) = primary.unsigned_attrs.as_ref() else {
    return Ok(None);
  };
  let mut repository = None;
  for value in attributes
    .iter()
    .filter(|attribute| attribute.oid == OID_COUNTERSIGNATURE)
    .flat_map(|attribute| attribute.values.iter())
  {
    let counter = value
      .decode_as::<SignerInfo>()
      .map_err(|error| signature_error(path, format!("invalid package countersignature: {error}")))?;
    if signature_kind(counter.signed_attrs.as_ref(), path)? == SignatureKind::Repository && repository.replace(counter).is_some() {
      return Err(signature_error(path, "package contains more than one repository countersignature"));
    }
  }
  Ok(repository)
}

fn match_trusted(policy: &SignaturePolicy, kind: SignatureKind, certificate_der: &[u8], actual_owners: &[String]) -> Option<bool> {
  let mut matched = None;
  for signer in policy.signers.iter() {
    let target_matches = matches!(
      (signer.kind, kind),
      (TrustedSignerKind::Author, SignatureKind::Author) | (TrustedSignerKind::Repository, SignatureKind::Repository)
    );
    let repository_matches =
      signer.kind != TrustedSignerKind::Repository || signer.owners.is_empty() || signer.owners.iter().any(|allowed| actual_owners.contains(allowed));
    if !target_matches || !repository_matches {
      continue;
    }
    for trusted in signer.certificates.iter() {
      let fingerprint_matches = match trusted.algorithm {
        FingerprintAlgorithm::Sha256 => Sha256::digest(certificate_der).as_slice() == trusted.fingerprint.as_ref(),
        FingerprintAlgorithm::Sha384 => Sha384::digest(certificate_der).as_slice() == trusted.fingerprint.as_ref(),
        FingerprintAlgorithm::Sha512 => Sha512::digest(certificate_der).as_slice() == trusted.fingerprint.as_ref(),
      };
      if fingerprint_matches {
        matched = Some(matched.unwrap_or(true) && trusted.allow_untrusted_root);
      }
    }
  }
  matched
}

fn verify_timestamp(path: &Path, signer: &SignerInfo, policy: Option<&SignaturePolicy>) -> Result<Option<SystemTime>, PackageError> {
  let Some(attributes) = signer.unsigned_attrs.as_ref() else {
    return Ok(None);
  };
  let values = attributes
    .iter()
    .filter(|attribute| attribute.oid == OID_TIMESTAMP_TOKEN)
    .flat_map(|attribute| attribute.values.iter())
    .collect::<Vec<_>>();
  if values.is_empty() {
    return Ok(None);
  }
  if values.len() > 1 {
    return Err(signature_error(path, "package signature contains more than one timestamp token"));
  }
  let content = values[0]
    .decode_as::<ContentInfo>()
    .map_err(|error| signature_error(path, format!("invalid package timestamp CMS content: {error}")))?;
  if content.content_type != OID_SIGNED_DATA {
    return Err(signature_error(path, "package timestamp CMS content type is not signed-data"));
  }
  let timestamp: SignedData = content
    .content
    .decode_as()
    .map_err(|error| signature_error(path, format!("invalid package timestamp signed-data: {error}")))?;
  if timestamp.signer_infos.0.len() != 1 || timestamp.encap_content_info.econtent_type != OID_TST_INFO {
    return Err(signature_error(path, "package timestamp must contain exactly one signer and TSTInfo content"));
  }
  let timestamp_content = timestamp
    .encap_content_info
    .econtent
    .as_ref()
    .ok_or_else(|| signature_error(path, "package timestamp has no TSTInfo content"))?
    .value()
    .to_vec();
  let timestamp_der = collect_certificate_der(&timestamp, path, "package timestamp")?;
  let parsed = ParsedCms {
    signed_data: timestamp,
    certificate_der: timestamp_der,
  };
  let timestamp_signer = parsed.signed_data.signer_infos.0.get(0).expect("timestamp signer count was checked");
  verify_cms_signer(path, timestamp_signer, &timestamp_content, &parsed, Some(OID_TST_INFO))?;
  let (imprint_algorithm, imprint, generated) = timestamp_info(&timestamp_content, path)?;
  let actual = digest(imprint_algorithm, signer.signature.as_bytes(), path)?;
  if actual != imprint {
    return Err(signature_error(path, "package timestamp message imprint does not match the signature"));
  }
  if let Some(policy) = policy {
    let certificate_index = signer_certificate_index(timestamp_signer, &parsed.signed_data, path)?;
    verify_certificate_chain(
      path,
      CertificateChainVerification {
        certificate_index,
        certificate: certificate(&parsed.signed_data, certificate_index),
        certificate_der: &parsed.certificate_der,
        time: generated,
        eku: TIME_STAMPING_EKU,
        allow_untrusted_root: false,
        configured_anchors: policy.timestamp_trust_anchors()?,
      },
    )?;
  }
  Ok(Some(generated))
}

fn timestamp_info(content: &[u8], path: &Path) -> Result<(ObjectIdentifier, Vec<u8>, SystemTime), PackageError> {
  let any = der::AnyRef::from_der(content).map_err(|error| signature_error(path, format!("invalid package TSTInfo: {error}")))?;
  any
    .sequence(|reader| {
      let _: der::AnyRef<'_> = reader.decode()?;
      let _: ObjectIdentifier = reader.decode()?;
      let imprint = reader.sequence(|reader| {
        let algorithm: spki::AlgorithmIdentifierOwned = reader.decode()?;
        let digest: OctetString = reader.decode()?;
        Ok((algorithm.oid, digest.as_bytes().to_vec()))
      })?;
      let _: der::AnyRef<'_> = reader.decode()?;
      let generated: GeneralizedTime = reader.decode()?;
      while !reader.is_finished() {
        let _: der::AnyRef<'_> = reader.decode()?;
      }
      Ok((imprint.0, imprint.1, generated.to_system_time()))
    })
    .map_err(|error| signature_error(path, format!("invalid package TSTInfo: {error}")))
}

struct CertificateChainVerification<'a> {
  certificate_index: usize,
  certificate: &'a Certificate,
  certificate_der: &'a [Vec<u8>],
  time: SystemTime,
  eku: &'static [u8],
  allow_untrusted_root: bool,
  configured_anchors: &'a [TrustAnchor<'static>],
}

fn verify_certificate_chain(path: &Path, verification: CertificateChainVerification<'_>) -> Result<(), PackageError> {
  let leaf_der = CertificateDer::from(verification.certificate_der[verification.certificate_index].as_slice());
  let leaf = EndEntityCert::try_from(&leaf_der).map_err(|error| signature_error(path, format!("invalid package signing certificate: {error}")))?;
  let intermediates = verification
    .certificate_der
    .iter()
    .enumerate()
    .filter(|(index, _)| *index != verification.certificate_index)
    .map(|(_, encoded)| CertificateDer::from(encoded.as_slice()))
    .collect::<Vec<_>>();
  let additional_anchors;
  let anchors = if verification.allow_untrusted_root {
    additional_anchors = untrusted_chain_anchors(verification.certificate_der, verification.certificate_index, verification.certificate);
    additional_anchors.as_slice()
  } else {
    verification.configured_anchors
  };
  let unix_time = UnixTime::since_unix_epoch(
    verification
      .time
      .duration_since(UNIX_EPOCH)
      .map_err(|_| signature_error(path, "certificate validation time predates the Unix epoch"))?,
  );
  leaf
    .verify_for_usage(
      ALL_VERIFICATION_ALGS,
      anchors,
      &intermediates,
      unix_time,
      KeyUsage::required(verification.eku),
      None,
      None,
    )
    .map_err(|error| signature_error(path, format!("package signing certificate chain is not trusted: {error}")))?;
  Ok(())
}

fn untrusted_chain_anchors(certificate_der: &[Vec<u8>], certificate_index: usize, leaf: &Certificate) -> Vec<TrustAnchor<'static>> {
  let parsed = certificate_der.iter().map(|encoded| Certificate::from_der(encoded).ok()).collect::<Vec<_>>();
  let mut anchors = Vec::new();
  for (index, encoded) in certificate_der.iter().enumerate() {
    let candidate = if index == certificate_index { Some(leaf) } else { parsed[index].as_ref() };
    let Some(candidate) = candidate else {
      continue;
    };
    let terminal = candidate.tbs_certificate.subject == candidate.tbs_certificate.issuer
      || !parsed
        .iter()
        .flatten()
        .any(|issuer| issuer.tbs_certificate.subject == candidate.tbs_certificate.issuer);
    if terminal && let Ok(anchor) = anchor_from_trusted_cert(&CertificateDer::from(encoded.as_slice())) {
      anchors.push(anchor.to_owned());
    }
  }
  if anchors.is_empty()
    && let Some(encoded) = certificate_der.get(certificate_index)
    && let Ok(anchor) = anchor_from_trusted_cert(&CertificateDer::from(encoded.as_slice()))
  {
    anchors.push(anchor.to_owned());
  }
  anchors
}

#[cfg(not(windows))]
fn load_pem_anchors(path: &Path) -> Result<Box<[TrustAnchor<'static>]>, String> {
  let bytes = fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
  let certificates = CertificateDer::pem_slice_iter(&bytes)
    .collect::<Result<Vec<_>, _>>()
    .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
  let anchors = certificates
    .iter()
    .filter_map(|certificate| anchor_from_trusted_cert(certificate).ok())
    .map(|anchor| anchor.to_owned())
    .collect::<Vec<_>>();
  if anchors.is_empty() {
    Err(format!("{} contains no usable certificate roots", path.display()))
  } else {
    Ok(anchors.into_boxed_slice())
  }
}

#[cfg(windows)]
fn platform_trust_anchors() -> Result<&'static [TrustAnchor<'static>], PackageError> {
  static ROOTS: OnceLock<Result<Box<[TrustAnchor<'static>]>, String>> = OnceLock::new();
  match ROOTS.get_or_init(|| {
    let result = rustls_native_certs::load_native_certs();
    let anchors = result
      .certs
      .iter()
      .filter_map(|certificate| anchor_from_trusted_cert(certificate).ok())
      .map(|anchor| anchor.to_owned())
      .collect::<Vec<_>>();
    if anchors.is_empty() {
      let detail = result
        .errors
        .first()
        .map_or_else(|| "the platform certificate store is empty".to_owned(), ToString::to_string);
      Err(detail)
    } else {
      Ok(anchors.into_boxed_slice())
    }
  }) {
    Ok(anchors) => Ok(anchors),
    Err(error) => Err(signature_error(
      "platform certificate store",
      format!("could not load trusted certificate roots: {error}"),
    )),
  }
}

#[derive(Clone, Copy)]
enum ContentHashAlgorithm {
  Sha256,
  Sha384,
  Sha512,
}

fn signature_content(content: &[u8], path: &Path) -> Result<(ContentHashAlgorithm, Vec<u8>), PackageError> {
  let content = std::str::from_utf8(content).map_err(|error| signature_error(path, format!("package signature content is not UTF-8: {error}")))?;
  let normalized = content.replace("\r\n", "\n");
  let mut sections = normalized.split("\n\n");
  let header = sections.next().unwrap_or_default();
  if !header.lines().any(|line| line == "Version:1") {
    return Err(signature_error(path, "package signature content has an unsupported format version"));
  }
  let hashes = sections
    .next()
    .ok_or_else(|| signature_error(path, "package signature content has no package hash section"))?;
  let (algorithm, encoded) = hashes
    .lines()
    .find_map(|line| {
      line
        .strip_prefix("2.16.840.1.101.3.4.2.1-Hash:")
        .map(|value| (ContentHashAlgorithm::Sha256, value))
        .or_else(|| {
          line
            .strip_prefix("2.16.840.1.101.3.4.2.2-Hash:")
            .map(|value| (ContentHashAlgorithm::Sha384, value))
        })
        .or_else(|| {
          line
            .strip_prefix("2.16.840.1.101.3.4.2.3-Hash:")
            .map(|value| (ContentHashAlgorithm::Sha512, value))
        })
    })
    .ok_or_else(|| signature_error(path, "package signature content uses no supported package hash"))?;
  let decoded = BASE64
    .decode(encoded)
    .map_err(|error| signature_error(path, format!("package signature content hash is not valid base64: {error}")))?;
  let expected_len = match algorithm {
    ContentHashAlgorithm::Sha256 => 32,
    ContentHashAlgorithm::Sha384 => 48,
    ContentHashAlgorithm::Sha512 => 64,
  };
  if decoded.len() != expected_len {
    return Err(signature_error(path, "package signature content hash has the wrong length"));
  }
  Ok((algorithm, decoded))
}

enum ArchiveHasher {
  Sha256(Sha256),
  Sha384(Sha384),
  Sha512(Sha512),
}

impl ArchiveHasher {
  fn new(algorithm: ContentHashAlgorithm) -> Self {
    match algorithm {
      ContentHashAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
      ContentHashAlgorithm::Sha384 => Self::Sha384(Sha384::new()),
      ContentHashAlgorithm::Sha512 => Self::Sha512(Sha512::new()),
    }
  }

  fn update(&mut self, bytes: &[u8]) {
    match self {
      Self::Sha256(hasher) => hasher.update(bytes),
      Self::Sha384(hasher) => hasher.update(bytes),
      Self::Sha512(hasher) => hasher.update(bytes),
    }
  }

  fn finish(self) -> Vec<u8> {
    match self {
      Self::Sha256(hasher) => hasher.finalize().to_vec(),
      Self::Sha384(hasher) => hasher.finalize().to_vec(),
      Self::Sha512(hasher) => hasher.finalize().to_vec(),
    }
  }
}

impl SignedArchive {
  fn read(file: &mut fs::File, path: &Path) -> Result<Option<Self>, PackageError> {
    let file_len = file.metadata().map_err(|error| signature_io(path, "inspect package", error))?.len();
    let footer_len = usize::try_from(file_len.min(65_557)).expect("bounded ZIP footer length fits usize");
    let footer_start = file_len - footer_len as u64;
    file
      .seek(SeekFrom::Start(footer_start))
      .map_err(|error| signature_io(path, "seek package footer", error))?;
    let mut footer = vec![0u8; footer_len];
    file.read_exact(&mut footer).map_err(|error| signature_io(path, "read package footer", error))?;
    let eocd_in_footer = (0..=footer_len.saturating_sub(22))
      .rev()
      .find(|offset| {
        footer[*offset..*offset + 4] == END_OF_CENTRAL_DIRECTORY_SIGNATURE && *offset + 22 + usize::from(u16_at(&footer, *offset + 20)) == footer_len
      })
      .ok_or_else(|| signature_error(path, "package ZIP has no valid end-of-central-directory record"))?;
    let eocd = footer_start + eocd_in_footer as u64;
    let disk = u16_at(&footer, eocd_in_footer + 4);
    let central_disk = u16_at(&footer, eocd_in_footer + 6);
    let entries_on_disk = u16_at(&footer, eocd_in_footer + 8);
    let entries_total = u16_at(&footer, eocd_in_footer + 10);
    let central_size = u64::from(u32_at(&footer, eocd_in_footer + 12));
    let central_start = u64::from(u32_at(&footer, eocd_in_footer + 16));
    if disk != 0 || central_disk != 0 || entries_on_disk != entries_total || entries_total == u16::MAX {
      return Err(signature_error(path, "signed packages must use a single-disk ZIP32 archive"));
    }
    if central_start.checked_add(central_size) != Some(eocd) {
      return Err(signature_error(path, "package ZIP central-directory bounds are inconsistent"));
    }

    file
      .seek(SeekFrom::Start(central_start))
      .map_err(|error| signature_io(path, "seek package central directory", error))?;
    let mut records = Vec::with_capacity(usize::from(entries_total));
    let mut signature_index = None;
    for _ in 0..entries_total {
      let position = file
        .stream_position()
        .map_err(|error| signature_io(path, "inspect package central directory", error))?;
      let mut fixed = [0u8; CENTRAL_DIRECTORY_FIXED_BYTES as usize];
      file
        .read_exact(&mut fixed)
        .map_err(|error| signature_io(path, "read package central directory", error))?;
      if u32_at(&fixed, 0) != CENTRAL_DIRECTORY_SIGNATURE {
        return Err(signature_error(path, "package ZIP contains a malformed central-directory record"));
      }
      let name_len = usize::from(u16_at(&fixed, 28));
      let extra_len = usize::from(u16_at(&fixed, 30));
      let comment_len = usize::from(u16_at(&fixed, 32));
      let header_size = CENTRAL_DIRECTORY_FIXED_BYTES
        .checked_add((name_len + extra_len + comment_len) as u64)
        .ok_or_else(|| signature_error(path, "package ZIP central-directory size overflow"))?;
      let is_signature = if name_len == SIGNATURE_PATH.len() {
        let mut name = [0u8; SIGNATURE_PATH.len()];
        file
          .read_exact(&mut name)
          .map_err(|error| signature_io(path, "read package entry name", error))?;
        name == SIGNATURE_PATH
      } else {
        file
          .seek(SeekFrom::Current(name_len as i64))
          .map_err(|error| signature_io(path, "skip package entry name", error))?;
        false
      };
      file
        .seek(SeekFrom::Current((extra_len + comment_len) as i64))
        .map_err(|error| signature_io(path, "skip package entry metadata", error))?;
      if is_signature && signature_index.replace(records.len()).is_some() {
        return Err(signature_error(path, "package contains more than one signature entry"));
      }
      records.push(CentralDirectoryRecord {
        position,
        local_offset: u64::from(u32_at(&fixed, 42)),
        file_entry_size: 0,
        header_size,
        change_in_offset: 0,
        flags: u16_at(&fixed, 8),
        compression: u16_at(&fixed, 10),
        compressed_size: u32_at(&fixed, 20),
        uncompressed_size: u32_at(&fixed, 24),
        external_attributes: u32_at(&fixed, 38),
      });
    }
    if file
      .stream_position()
      .map_err(|error| signature_io(path, "inspect package central directory", error))?
      != eocd
    {
      return Err(signature_error(path, "package ZIP central-directory record count is inconsistent"));
    }
    let Some(signature_index) = signature_index else {
      return Ok(None);
    };

    let mut local_order = (0..records.len()).collect::<Vec<_>>();
    local_order.sort_unstable_by_key(|index| records[*index].local_offset);
    for (position, index) in local_order.iter().copied().enumerate() {
      let next = local_order.get(position + 1).map_or(central_start, |next| records[*next].local_offset);
      records[index].file_entry_size = next
        .checked_sub(records[index].local_offset)
        .ok_or_else(|| signature_error(path, "package ZIP local records overlap"))?;
    }
    let start_of_local_headers = local_order.first().map_or(central_start, |index| records[*index].local_offset);

    let signature = &records[signature_index];
    if signature.flags != 0
      || signature.compression != 0
      || signature.compressed_size != signature.uncompressed_size
      || signature.external_attributes != 0
      || u64::from(signature.compressed_size) > MAX_SIGNATURE_BYTES
    {
      return Err(signature_error(path, "package signature entry violates the NuGet signing ZIP layout"));
    }
    validate_signature_local_header(file, signature, path)?;

    let mut previous_unsigned_end = 0i64;
    for index in local_order {
      if index == signature_index {
        continue;
      }
      let local_offset = i64::try_from(records[index].local_offset).map_err(|_| signature_error(path, "package ZIP offset exceeds i64"))?;
      records[index].change_in_offset = previous_unsigned_end - local_offset;
      previous_unsigned_end = local_offset
        .checked_add(i64::try_from(records[index].file_entry_size).map_err(|_| signature_error(path, "package ZIP record exceeds i64"))?)
        .and_then(|value| value.checked_add(records[index].change_in_offset))
        .ok_or_else(|| signature_error(path, "package ZIP offset overflow"))?;
    }
    Ok(Some(Self {
      records,
      signature_index,
      start_of_local_headers,
      eocd,
      file_len,
    }))
  }

  fn read_signature(&self, file: &mut fs::File, path: &Path) -> Result<Vec<u8>, PackageError> {
    let record = &self.records[self.signature_index];
    file
      .seek(SeekFrom::Start(record.local_offset))
      .map_err(|error| signature_io(path, "seek package signature", error))?;
    let mut fixed = [0u8; LOCAL_FILE_FIXED_BYTES as usize];
    file
      .read_exact(&mut fixed)
      .map_err(|error| signature_io(path, "read package signature header", error))?;
    let data_offset = record
      .local_offset
      .checked_add(LOCAL_FILE_FIXED_BYTES)
      .and_then(|value| value.checked_add(u64::from(u16_at(&fixed, 26))))
      .and_then(|value| value.checked_add(u64::from(u16_at(&fixed, 28))))
      .ok_or_else(|| signature_error(path, "package signature data offset overflow"))?;
    file
      .seek(SeekFrom::Start(data_offset))
      .map_err(|error| signature_io(path, "seek package signature data", error))?;
    let mut signature = vec![0u8; record.compressed_size as usize];
    file
      .read_exact(&mut signature)
      .map_err(|error| signature_io(path, "read package signature data", error))?;
    Ok(signature)
  }

  fn unsigned_hash(&self, file: &mut fs::File, algorithm: ContentHashAlgorithm, path: &Path) -> Result<Vec<u8>, PackageError> {
    let signature = &self.records[self.signature_index];
    let mut hasher = ArchiveHasher::new(algorithm);
    let mut buffer = [0u8; 64 * 1024];
    hash_range(file, &mut hasher, 0, self.start_of_local_headers, &mut buffer, path)?;

    let mut local = self
      .records
      .iter()
      .enumerate()
      .filter(|(index, _)| *index != self.signature_index)
      .collect::<Vec<_>>();
    local.sort_unstable_by_key(|(_, record)| record.local_offset);
    for (_, record) in local {
      hash_range(file, &mut hasher, record.local_offset, record.file_entry_size, &mut buffer, path)?;
    }

    let mut central = self
      .records
      .iter()
      .enumerate()
      .filter(|(index, _)| *index != self.signature_index)
      .collect::<Vec<_>>();
    central.sort_unstable_by_key(|(_, record)| record.position);
    for (_, record) in central {
      if record.header_size < CENTRAL_DIRECTORY_FIXED_BYTES {
        return Err(signature_error(path, "package ZIP central-directory record is truncated"));
      }
      hash_range(file, &mut hasher, record.position, 42, &mut buffer, path)?;
      let original = read_u32(file, record.position + 42, path)?;
      let patched = i64::from(original)
        .checked_add(record.change_in_offset)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| signature_error(path, "package ZIP local offset cannot be represented in ZIP32"))?;
      hasher.update(&patched.to_le_bytes());
      hash_range(file, &mut hasher, record.position + 46, record.header_size - 46, &mut buffer, path)?;
    }

    file
      .seek(SeekFrom::Start(self.eocd))
      .map_err(|error| signature_io(path, "seek package footer", error))?;
    let mut footer = [0u8; 20];
    file.read_exact(&mut footer).map_err(|error| signature_io(path, "read package footer", error))?;
    if footer[..4] != END_OF_CENTRAL_DIRECTORY_SIGNATURE {
      return Err(signature_error(path, "package ZIP footer changed during verification"));
    }
    hasher.update(&footer[..8]);
    let entries_on_disk = u16_at(&footer, 8)
      .checked_sub(1)
      .ok_or_else(|| signature_error(path, "package ZIP entry count underflow"))?;
    let entries_total = u16_at(&footer, 10)
      .checked_sub(1)
      .ok_or_else(|| signature_error(path, "package ZIP entry count underflow"))?;
    hasher.update(&entries_on_disk.to_le_bytes());
    hasher.update(&entries_total.to_le_bytes());
    let central_size = u32_at(&footer, 12)
      .checked_sub(u32::try_from(signature.header_size).map_err(|_| signature_error(path, "package signature central record exceeds ZIP32"))?)
      .ok_or_else(|| signature_error(path, "package ZIP central size underflow"))?;
    hasher.update(&central_size.to_le_bytes());
    let central_offset = u32_at(&footer, 16)
      .checked_sub(u32::try_from(signature.file_entry_size).map_err(|_| signature_error(path, "package signature local record exceeds ZIP32"))?)
      .ok_or_else(|| signature_error(path, "package ZIP central offset underflow"))?;
    hasher.update(&central_offset.to_le_bytes());
    hash_range(file, &mut hasher, self.eocd + 20, self.file_len - self.eocd - 20, &mut buffer, path)?;
    Ok(hasher.finish())
  }
}

fn validate_signature_local_header(file: &mut fs::File, record: &CentralDirectoryRecord, path: &Path) -> Result<(), PackageError> {
  file
    .seek(SeekFrom::Start(record.local_offset))
    .map_err(|error| signature_io(path, "seek package signature header", error))?;
  let mut fixed = [0u8; LOCAL_FILE_FIXED_BYTES as usize];
  file
    .read_exact(&mut fixed)
    .map_err(|error| signature_io(path, "read package signature header", error))?;
  if u32_at(&fixed, 0) != LOCAL_FILE_SIGNATURE
    || u16_at(&fixed, 6) != 0
    || u16_at(&fixed, 8) != 0
    || u32_at(&fixed, 18) != record.compressed_size
    || u32_at(&fixed, 22) != record.uncompressed_size
    || usize::from(u16_at(&fixed, 26)) != SIGNATURE_PATH.len()
  {
    return Err(signature_error(path, "package signature local header violates the NuGet signing ZIP layout"));
  }
  let mut name = [0u8; SIGNATURE_PATH.len()];
  file
    .read_exact(&mut name)
    .map_err(|error| signature_io(path, "read package signature name", error))?;
  if name != SIGNATURE_PATH {
    return Err(signature_error(path, "package signature local-header name is invalid"));
  }
  Ok(())
}

fn hash_range(file: &mut fs::File, hasher: &mut ArchiveHasher, offset: u64, mut length: u64, buffer: &mut [u8], path: &Path) -> Result<(), PackageError> {
  file
    .seek(SeekFrom::Start(offset))
    .map_err(|error| signature_io(path, "seek signed package", error))?;
  while length != 0 {
    let count = usize::try_from(length.min(buffer.len() as u64)).expect("bounded archive read fits usize");
    file
      .read_exact(&mut buffer[..count])
      .map_err(|error| signature_io(path, "read signed package", error))?;
    hasher.update(&buffer[..count]);
    length -= count as u64;
  }
  Ok(())
}

fn read_u32(file: &mut fs::File, offset: u64, path: &Path) -> Result<u32, PackageError> {
  file
    .seek(SeekFrom::Start(offset))
    .map_err(|error| signature_io(path, "seek signed package", error))?;
  let mut bytes = [0u8; 4];
  file.read_exact(&mut bytes).map_err(|error| signature_io(path, "read signed package", error))?;
  Ok(u32::from_le_bytes(bytes))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
  u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("fixed ZIP field is in bounds"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
  u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed ZIP field is in bounds"))
}

fn signature_io(path: &Path, action: &str, error: io::Error) -> PackageError {
  PackageError::new(PackageErrorKind::Io, path.display().to_string(), format!("failed to {action}: {error}"))
}

fn signature_error(context: impl AsRef<Path>, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Integrity, context.as_ref().display().to_string(), message)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Write;

  #[test]
  fn official_author_signed_fixture_verifies_crypto_and_integrity() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/author-signed.nupkg");
    let policy = SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new());
    assert!(verify_package(&path, &policy).unwrap());
  }

  #[cfg(windows)]
  #[test]
  fn official_author_signed_fixture_matches_trusted_author_and_platform_roots() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/author-signed.nupkg");
    let certificate = TrustedCertificate::parse(
      "3F9001EA83C560D712C24CF213C3D312CB3BFF51EE89435D3430BD06B5D0EECE",
      FingerprintAlgorithm::Sha256,
      false,
    )
    .unwrap();
    let policy = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "Microsoft".to_owned(),
        service_index: None,
        owners: Box::new([]),
        certificates: vec![certificate].into_boxed_slice(),
        kind: TrustedSignerKind::Author,
      }],
    );

    assert!(verify_package(&path, &policy).unwrap());
  }

  #[test]
  fn signed_fixture_rejects_an_untrusted_author() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/author-signed.nupkg");
    let certificate = TrustedCertificate::parse(&"00".repeat(32), FingerprintAlgorithm::Sha256, true).unwrap();
    let policy = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "not-the-signer".to_owned(),
        service_index: None,
        owners: Box::new([]),
        certificates: vec![certificate].into_boxed_slice(),
        kind: TrustedSignerKind::Author,
      }],
    );

    let error = verify_package(&path, &policy).unwrap_err();
    assert!(error.to_string().contains("does not match trustedSigners"));
  }

  #[cfg(windows)]
  #[test]
  fn repository_signed_fixture_matches_trusted_repository_and_platform_roots() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/repository-signed.nupkg");
    let certificate = TrustedCertificate::parse(
      "1F4B311D9ACC115C8DC8018B5A49E00FCE6DA8E2855F9F014CA6F34570BC482D",
      FingerprintAlgorithm::Sha256,
      false,
    )
    .unwrap();
    let wrong_owner = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "nuget.org".to_owned(),
        service_index: Some("https://api.nuget.org/v3/index.json".to_owned()),
        owners: vec!["definitely-not-a-package-owner".to_owned()].into_boxed_slice(),
        certificates: vec![certificate.clone()].into_boxed_slice(),
        kind: TrustedSignerKind::Repository,
      }],
    );
    assert!(
      verify_package(&path, &wrong_owner)
        .unwrap_err()
        .to_string()
        .contains("does not match trustedSigners")
    );
    let policy = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "nuget.org".to_owned(),
        service_index: Some("https://api.nuget.org/v3/index.json".to_owned()),
        owners: Box::new([]),
        certificates: vec![certificate].into_boxed_slice(),
        kind: TrustedSignerKind::Repository,
      }],
    );

    assert!(verify_package(&path, &policy).unwrap());
  }

  #[test]
  fn trusted_certificate_fingerprints_are_strict() {
    let fingerprint = "ab".repeat(32);
    let parsed = TrustedCertificate::parse(&fingerprint, FingerprintAlgorithm::Sha256, false).unwrap();
    assert_eq!(parsed.fingerprint.as_ref(), &[0xab; 32]);
    assert!(TrustedCertificate::parse("ab", FingerprintAlgorithm::Sha256, false).is_err());
  }

  #[test]
  fn require_rejects_unsigned_packages() {
    let path = temporary_package_path("unsigned");
    let file = fs::File::create(&path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive.start_file("Unsigned.nuspec", zip::write::SimpleFileOptions::default()).unwrap();
    archive.write_all(b"<package />").unwrap();
    archive.finish().unwrap();
    let policy = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "unused".to_owned(),
        service_index: None,
        owners: Box::new([]),
        certificates: vec![TrustedCertificate::parse(&"00".repeat(32), FingerprintAlgorithm::Sha256, true).unwrap()].into_boxed_slice(),
        kind: TrustedSignerKind::Author,
      }],
    );

    let error = verify_package(&path, &policy).unwrap_err();
    fs::remove_file(&path).unwrap();
    assert!(error.to_string().contains("package is unsigned"));
  }

  #[test]
  fn signed_archive_tampering_fails_the_content_hash() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/author-signed.nupkg");
    let path = temporary_package_path("tampered");
    fs::copy(fixture, &path).unwrap();
    let mut file = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(100)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(100)).unwrap();
    file.write_all(&[byte[0] ^ 0xff]).unwrap();
    drop(file);

    let error = verify_package(&path, &SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new())).unwrap_err();
    fs::remove_file(&path).unwrap();
    assert!(error.to_string().contains("content hash does not match"));
  }

  #[test]
  fn trusted_repository_owners_are_case_sensitive_and_conflicting_root_flags_choose_false() {
    let certificate_der = b"certificate";
    let fingerprint = Sha256::digest(certificate_der).to_vec().into_boxed_slice();
    let certificate = |allow_untrusted_root| TrustedCertificate {
      fingerprint: fingerprint.clone(),
      algorithm: FingerprintAlgorithm::Sha256,
      allow_untrusted_root,
    };
    let signer = |owner: &str, allow_untrusted_root| TrustedSigner {
      name: "repository".to_owned(),
      service_index: Some("https://configured.example/v3/index.json".to_owned()),
      owners: vec![owner.to_owned()].into_boxed_slice(),
      certificates: vec![certificate(allow_untrusted_root)].into_boxed_slice(),
      kind: TrustedSignerKind::Repository,
    };
    let wrong_case = SignaturePolicy::new(SignatureValidationMode::Require, vec![signer("Owner", true)]);
    assert_eq!(
      match_trusted(&wrong_case, SignatureKind::Repository, certificate_der, &["owner".to_owned()]),
      None
    );

    let conflicting = SignaturePolicy::new(SignatureValidationMode::Require, vec![signer("owner", true), signer("owner", false)]);
    assert_eq!(
      match_trusted(&conflicting, SignatureKind::Repository, certificate_der, &["owner".to_owned()]),
      Some(false)
    );
  }

  fn temporary_package_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("dv-signature-{label}-{}-{nonce}.nupkg", std::process::id()))
  }
}
