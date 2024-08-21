use crate::{
    aead::{Aead, AesGcm128, AesGcm256, ChaCha20Poly1305, ExportOnlyAead},
    kdf::{HkdfSha256, HkdfSha384, HkdfSha512, Kdf as KdfTrait},
    kem::{
        self, DhK256HkdfSha256, DhP256HkdfSha256, DhP384HkdfSha384, DhP521HkdfSha512,
        Kem as KemTrait, SharedSecret, X25519HkdfSha256,
    },
    op_mode::{OpModeR, PskBundle},
    setup::setup_receiver,
    Deserializable, HpkeError, Serializable,
};

extern crate std;
use std::{fs::File, string::String, vec::Vec};

use hex;
use serde::{de::Error as SError, Deserialize, Deserializer};
use serde_json;

// For known-answer tests we need to be able to encap with fixed randomness. This allows that.
trait TestableKem: KemTrait {
    /// The ephemeral key used in encapsulation. This is the same thing as a private key in the
    /// case of DHKEM, but this is not always true
    type EphemeralKey: Deserializable;

    // Encap with fixed randomness
    #[doc(hidden)]
    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError>;
}

// Now implement TestableKem for all the KEMs in the KAT
impl TestableKem for X25519HkdfSha256 {
    // In DHKEM, ephemeral keys and private keys are both scalars
    type EphemeralKey = <X25519HkdfSha256 as KemTrait>::PrivateKey;

    // Call the x25519 deterministic encap function we defined in dhkem.rs
    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError> {
        kem::x25519_hkdfsha256::encap_with_eph(pk_recip, sender_id_keypair, sk_eph)
    }
}
impl TestableKem for DhP256HkdfSha256 {
    // In DHKEM, ephemeral keys and private keys are both scalars
    type EphemeralKey = <DhP256HkdfSha256 as KemTrait>::PrivateKey;

    // Call the p256 deterministic encap function we defined in dhkem.rs
    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError> {
        kem::dhp256_hkdfsha256::encap_with_eph(pk_recip, sender_id_keypair, sk_eph)
    }
}

impl TestableKem for DhP384HkdfSha384 {
    // In DHKEM, ephemeral keys and private keys are both scalars
    type EphemeralKey = <DhP384HkdfSha384 as KemTrait>::PrivateKey;

    // Call the p384 deterministic encap function we defined in dhkem.rs
    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError> {
        kem::dhp384_hkdfsha384::encap_with_eph(pk_recip, sender_id_keypair, sk_eph)
    }
}

impl TestableKem for DhP521HkdfSha512 {
    // In DHKEM, ephemeral keys and private keys are both scalars
    type EphemeralKey = <DhP521HkdfSha512 as KemTrait>::PrivateKey;

    // Call the p521 deterministic encap function we defined in dhkem.rs
    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError> {
        kem::dhp521_hkdfsha512::encap_with_eph(pk_recip, sender_id_keypair, sk_eph)
    }
}

impl TestableKem for DhK256HkdfSha256 {
    type EphemeralKey = <DhK256HkdfSha256 as KemTrait>::PrivateKey;

    fn encap_with_eph(
        pk_recip: &Self::PublicKey,
        sender_id_keypair: Option<(&Self::PrivateKey, &Self::PublicKey)>,
        sk_eph: Self::EphemeralKey,
    ) -> Result<(SharedSecret<Self>, Self::EncappedKey), HpkeError> {
        kem::dhk256_hkdfsha256::encap_with_eph(pk_recip, sender_id_keypair, sk_eph)
    }
}

/// Asserts that the given serializable values are equal
macro_rules! assert_serializable_eq {
    ($a:expr, $b:expr, $args:tt) => {
        assert_eq!($a.to_bytes(), $b.to_bytes(), $args)
    };
}

// Tells serde how to deserialize bytes from the hex representation
fn bytes_from_hex<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let mut hex_str = String::deserialize(deserializer)?;
    // Prepend a 0 if it's not even length
    if hex_str.len() % 2 == 1 {
        hex_str.insert(0, '0');
    }
    hex::decode(hex_str).map_err(|e| SError::custom(format!("{:?}", e)))
}

// Tells serde how to deserialize bytes from an optional field with hex encoding
fn bytes_from_hex_opt<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    bytes_from_hex(deserializer).map(|v| Some(v))
}

// Each individual test case looks like this
#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
struct MainTestVector {
    // Parameters
    mode: u8,
    kem_id: u16,
    kdf_id: u16,
    aead_id: u16,
    #[serde(deserialize_with = "bytes_from_hex")]
    info: Vec<u8>,

    // Keying material
    #[serde(rename = "ikmR", deserialize_with = "bytes_from_hex")]
    ikm_recip: Vec<u8>,
    #[serde(default, rename = "ikmS", deserialize_with = "bytes_from_hex_opt")]
    ikm_sender: Option<Vec<u8>>,
    #[serde(rename = "ikmE", deserialize_with = "bytes_from_hex")]
    _ikm_eph: Vec<u8>,

    // Private keys
    #[serde(rename = "skRm", deserialize_with = "bytes_from_hex")]
    sk_recip: Vec<u8>,
    #[serde(default, rename = "skSm", deserialize_with = "bytes_from_hex_opt")]
    sk_sender: Option<Vec<u8>>,
    #[serde(rename = "skEm", deserialize_with = "bytes_from_hex")]
    sk_eph: Vec<u8>,

    // Preshared Key Bundle
    #[serde(default, deserialize_with = "bytes_from_hex_opt")]
    psk: Option<Vec<u8>>,
    #[serde(default, rename = "psk_id", deserialize_with = "bytes_from_hex_opt")]
    psk_id: Option<Vec<u8>>,

    // Public Keys
    #[serde(rename = "pkRm", deserialize_with = "bytes_from_hex")]
    pk_recip: Vec<u8>,
    #[serde(default, rename = "pkSm", deserialize_with = "bytes_from_hex_opt")]
    pk_sender: Option<Vec<u8>>,
    #[serde(rename = "pkEm", deserialize_with = "bytes_from_hex")]
    _pk_eph: Vec<u8>,

    // Key schedule inputs and computations
    #[serde(rename = "enc", deserialize_with = "bytes_from_hex")]
    encapped_key: Vec<u8>,
    #[serde(deserialize_with = "bytes_from_hex")]
    shared_secret: Vec<u8>,
    #[serde(rename = "key_schedule_context", deserialize_with = "bytes_from_hex")]
    _hpke_context: Vec<u8>,
    #[serde(rename = "secret", deserialize_with = "bytes_from_hex")]
    _key_schedule_secret: Vec<u8>,
    #[serde(rename = "key", deserialize_with = "bytes_from_hex")]
    _aead_key: Vec<u8>,
    #[serde(rename = "base_nonce", deserialize_with = "bytes_from_hex")]
    _aead_base_nonce: Vec<u8>,
    #[serde(rename = "exporter_secret", deserialize_with = "bytes_from_hex")]
    _exporter_secret: Vec<u8>,

    encryptions: Vec<EncryptionTestVector>,
    exports: Vec<ExporterTestVector>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
struct EncryptionTestVector {
    #[serde(rename = "pt", deserialize_with = "bytes_from_hex")]
    plaintext: Vec<u8>,
    #[serde(deserialize_with = "bytes_from_hex")]
    aad: Vec<u8>,
    #[serde(rename = "nonce", deserialize_with = "bytes_from_hex")]
    _nonce: Vec<u8>,
    #[serde(rename = "ct", deserialize_with = "bytes_from_hex")]
    ciphertext: Vec<u8>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
struct ExporterTestVector {
    #[serde(rename = "exporter_context", deserialize_with = "bytes_from_hex")]
    export_ctx: Vec<u8>,
    #[serde(rename = "L")]
    export_len: usize,
    #[serde(rename = "exported_value", deserialize_with = "bytes_from_hex")]
    export_val: Vec<u8>,
}

/// Returns a keypair given the secret bytes and pubkey bytes
fn deser_keypair<Kem: KemTrait>(
    sk_bytes: &[u8],
    pk_bytes: &[u8],
) -> (Kem::PrivateKey, Kem::PublicKey) {
    // Deserialize the secret key
    let sk = <Kem as KemTrait>::PrivateKey::from_bytes(sk_bytes).unwrap();
    // Deserialize the pubkey
    let pk = <Kem as KemTrait>::PublicKey::from_bytes(pk_bytes).unwrap();

    (sk, pk)
}

/// Constructs an `OpModeR` from the given components. The variant constructed is determined solely
/// by `mode_id`. This will panic if there is insufficient data to construct the variants specified
/// by `mode_id`.
fn make_op_mode_r<'a, Kem: KemTrait>(
    mode_id: u8,
    pk: Option<Kem::PublicKey>,
    psk: Option<&'a [u8]>,
    psk_id: Option<&'a [u8]>,
) -> OpModeR<'a, Kem> {
    // Deserialize the optional bundle
    let bundle = psk.map(|bytes| PskBundle {
        psk: bytes,
        psk_id: psk_id.unwrap(),
    });

    // These better be set if the mode ID calls for them
    match mode_id {
        0 => OpModeR::Base,
        1 => OpModeR::Psk(bundle.unwrap()),
        2 => OpModeR::Auth(pk.unwrap()),
        3 => OpModeR::AuthPsk(pk.unwrap(), bundle.unwrap()),
        _ => panic!("Invalid mode ID: {}", mode_id),
    }
}

// This does all the legwork
fn test_case<A: Aead, Kdf: KdfTrait, Kem: TestableKem>(tv: MainTestVector) {
    // First, deserialize all the relevant keys so we can reconstruct the encapped key
    let recip_keypair = deser_keypair::<Kem>(&tv.sk_recip, &tv.pk_recip);
    let sk_eph = <Kem as TestableKem>::EphemeralKey::from_bytes(&tv.sk_eph).unwrap();
    let sender_keypair = {
        let pk_sender = &tv.pk_sender.as_ref();
        tv.sk_sender
            .as_ref()
            .map(|sk| deser_keypair::<Kem>(sk, pk_sender.unwrap()))
    };

    // Make sure the keys match what we would've gotten had we used DeriveKeyPair
    {
        let derived_kp = Kem::derive_keypair(&tv.ikm_recip);
        assert_serializable_eq!(recip_keypair.0, derived_kp.0, "sk recip doesn't match");
        assert_serializable_eq!(recip_keypair.1, derived_kp.1, "pk recip doesn't match");
    }
    if let Some(sks) = sender_keypair.as_ref() {
        let derived_kp = Kem::derive_keypair(&tv.ikm_sender.unwrap());
        assert_serializable_eq!(sks.0, derived_kp.0, "sk sender doesn't match");
        assert_serializable_eq!(sks.1, derived_kp.1, "pk sender doesn't match");
    }

    let (sk_recip, pk_recip) = recip_keypair;

    // Now derive the encapped key with the deterministic encap function, using all the inputs
    // above
    let (shared_secret, encapped_key) = {
        let sender_keypair_ref = sender_keypair.as_ref().map(|&(ref sk, ref pk)| (sk, pk));
        Kem::encap_with_eph(&pk_recip, sender_keypair_ref, sk_eph).expect("encap failed")
    };

    // Assert that the derived shared secret key is identical to the one provided
    assert_eq!(
        shared_secret.0.as_slice(),
        tv.shared_secret.as_slice(),
        "shared_secret doesn't match"
    );

    // Assert that the derived encapped key is identical to the one provided
    {
        let provided_encapped_key =
            <Kem as KemTrait>::EncappedKey::from_bytes(&tv.encapped_key).unwrap();
        assert_serializable_eq!(
            encapped_key,
            provided_encapped_key,
            "encapped keys don't match"
        );
    }

    // We're going to test the encryption contexts. First, construct the appropriate OpMode.
    let mode = make_op_mode_r(
        tv.mode,
        sender_keypair.map(|(_, pk)| pk),
        tv.psk.as_ref().map(Vec::as_slice),
        tv.psk_id.as_ref().map(Vec::as_slice),
    );
    let mut aead_ctx = setup_receiver::<A, Kdf, Kem>(&mode, &sk_recip, &encapped_key, &tv.info)
        .expect("setup_receiver failed");

    // Go through all the plaintext-ciphertext pairs of this test vector and assert the
    // ciphertext decrypts to the corresponding plaintext
    for enc_packet in tv.encryptions {
        // Descructure the vector
        let EncryptionTestVector {
            aad,
            ciphertext,
            plaintext,
            ..
        } = enc_packet;

        // Open the ciphertext and assert that it succeeds
        let decrypted = aead_ctx.open(&ciphertext, &aad).expect("open failed");

        // Assert the decrypted payload equals the expected plaintext
        assert_eq!(decrypted, plaintext, "plaintexts don't match");
    }

    // Now check that AeadCtx::export returns the expected values
    for export in tv.exports {
        let mut exported_val = vec![0u8; export.export_len];
        aead_ctx
            .export(&export.export_ctx, &mut exported_val)
            .unwrap();
        assert_eq!(exported_val, export.export_val, "export values don't match");
    }
}

// This macro takes in all the supported AEADs, KDFs, and KEMs, and dispatches the given test
// vector to the test case with the appropriate types
macro_rules! dispatch_testcase {
    // Step 1: Roll up the AEAD, KDF, and KEM types into tuples. We'll unroll them later
    ($tv:ident, ($( $aead_ty:ty ),*), ($( $kdf_ty:ty ),*), ($( $kem_ty:ty ),*)) => {
        dispatch_testcase!(@tup1 $tv, ($( $aead_ty ),*), ($( $kdf_ty ),*), ($( $kem_ty ),*))
    };
    // Step 2: Expand with respect to every AEAD
    (@tup1 $tv:ident, ($( $aead_ty:ty ),*), $kdf_tup:tt, $kem_tup:tt) => {
        $(
            dispatch_testcase!(@tup2 $tv, $aead_ty, $kdf_tup, $kem_tup);
        )*
    };
    // Step 3: Expand with respect to every KDF
    (@tup2 $tv:ident, $aead_ty:ty, ($( $kdf_ty:ty ),*), $kem_tup:tt) => {
        $(
            dispatch_testcase!(@tup3 $tv, $aead_ty, $kdf_ty, $kem_tup);
        )*
    };
    // Step 4: Expand with respect to every KEM
    (@tup3 $tv:ident, $aead_ty:ty, $kdf_ty:ty, ($( $kem_ty:ty ),*)) => {
        $(
            dispatch_testcase!(@base $tv, $aead_ty, $kdf_ty, $kem_ty);
        )*
    };
    // Step 5: Now that we're only dealing with 1 type of each kind, do the dispatch. If the test
    // vector matches the IDs of these types, run the test case.
    (@base $tv:ident, $aead_ty:ty, $kdf_ty:ty, $kem_ty:ty) => {
        if let (<$aead_ty>::AEAD_ID, <$kdf_ty>::KDF_ID, <$kem_ty>::KEM_ID) =
            ($tv.aead_id, $tv.kdf_id, $tv.kem_id)
        {
            println!(
                "Running test case on {}, {}, {}",
                stringify!($aead_ty),
                stringify!($kdf_ty),
                stringify!($kem_ty)
            );

            let tv = $tv.clone();
            test_case::<$aead_ty, $kdf_ty, $kem_ty>(tv);

            // This is so that code that comes after a dispatch_testcase! invocation will know that
            // the test vector matched no known ciphersuites
            continue;
        }
    };
}

pub mod gen {
    use std::io::Write;

    use generic_array::GenericArray;
    use rand::{CryptoRng, RngCore};

    use super::*;

    use crate::{aead::AeadTag, kdf::{labeled_extract, LabeledExpand}, setup::ExporterSecret, setup_sender, OpModeS};

    const EXPORT_LEN: usize = 32;
    const EXPORTER_CONTEXTS: [&[u8]; 3] = [b"", b"00", b"54657374436f6e74657874"];
    const OP_MODES: [u8; 4] = [0x00, 0x01, 0x02, 0x03];
    const SHA256_NH: usize = 32;

    enum OpMode {
        Base,
        Psk,
        Auth,
        AuthPsk,
    }

    struct PskExtras<Kem: TestableKem> {
        sender_keypair: (Kem::PrivateKey, Kem::PublicKey),
        ikm: GenericArray<u8, <Kem::PrivateKey as Serializable>::OutputSize>,
        psk: Vec<u8>,
        psk_id: Vec<u8>,
    }

    /// This does all the legwork
    fn gen_test_case<A: Aead, Kdf: KdfTrait, Kem: TestableKem, R: CryptoRng + RngCore>(mode: u8, csprng: &mut R) -> MainTestVector {
        let info = b"4f6465206f6e2061204772656369616e2055726e"; // same as RFC 9180 test vectors
        let ikm_eph = gen_ikm::<Kem, R>(csprng);
        let (sk_eph, pk_eph) = Kem::derive_keypair(&ikm_eph);
        let ikm_recip = gen_ikm::<Kem, R>(csprng);
        let recip_keypair = Kem::derive_keypair(&ikm_recip);
        
        // for loop through the modes
        let psk_extras: Option<PskExtras<Kem>> = match mode {
            0x01
            | 0x03 => {
                let ikm_sender = gen_ikm::<Kem, R>(csprng);
                Some(PskExtras {
                    sender_keypair: Kem::derive_keypair(&ikm_sender),
                    ikm: ikm_sender,
                    psk: vec![],
                    psk_id: vec![],
                })
            },
            _ => None,
        };
        // TODO gen ikmE, skEm, ikmR, skRm, base_nonce, key, from rand
        // TODO pk_em := enc

        let (sk_recip, pk_recip) = recip_keypair;

        // Now derive the encapped key with the deterministic encap function, using all the inputs
        // above
        let (shared_secret, encapped_key) = {
            let sender_keypair_ref = psk_extras.as_ref().map(|extras| extras.sender_keypair).map(|(ref sk, ref pk)| (sk, pk));
            // get sk_eph as TestableKem, it's a Kem generally
            let sk_eph = <Kem as TestableKem>::EphemeralKey::from_bytes(&sk_eph.to_bytes()).unwrap();
            Kem::encap_with_eph(&pk_recip, sender_keypair_ref, sk_eph).expect("encap failed")
        };

        // "enc"
        let encapped_key = <Kem as KemTrait>::EncappedKey::from_bytes(&pk_eph.to_bytes()).unwrap();

        let suite_id: [u8; 10] = crate::util::full_suite_id::<A, Kdf, Kem>();

        // generate key_schedule_context for PSK modes
        let (psk_id_hash, _) =
            labeled_extract::<Kdf>(&[], &suite_id, b"psk_id_hash", &[mode]);
        let (info_hash, _) = labeled_extract::<Kdf>(&[], &suite_id, b"info_hash", info);

        let mut key_schedule_context = Vec::new();
        key_schedule_context.extend_from_slice(&[mode]);
        key_schedule_context.extend_from_slice(&psk_id_hash);
        key_schedule_context.extend_from_slice(&info_hash);

        let psk = psk_extras.as_ref().map(|extras| extras.psk.as_slice());

        // set up mode_s
        let mode_s = make_op_mode_s::<Kem>(mode, psk_extras.map(|e| e.sender_keypair), psk_extras.map(|e| e.psk.as_slice()), psk_extras.map(|e| e.psk_id.as_slice()));
        let (_encapped_key, mut aead_ctx_s) = setup_sender::<A, Kdf, Kem, R>(&mode_s, &pk_recip, info, csprng).expect("Sender setup failed");
        

        // We're going to test the encryption contexts. First, construct the appropriate OpMode.
        let mode_r = make_op_mode_r::<Kem>(
            0x00, // TODO iterate all
            psk_extras.map(|extras| &extras.sender_keypair).map(|(_sk, pk)| *pk),
            psk,
            psk_extras.as_ref().map(|extras| extras.psk_id.as_slice()),
        );


        let mut aead_ctx_r = setup_receiver::<A, Kdf, Kem>(&mode_r, &sk_recip, &encapped_key, info)
            .expect("setup_receiver failed");

        // TODO derive secret, key, base_nonce, exporter_secret

        // let secret = labeled_extract::<Kdf>(&shared_secret.0, &suite_id, b"secret", psk.unwrap_or(&[]));

        // // Derive the key
        // let mut key = vec![0u8; Nk];
        // labeled_expand::<Kdf>(&secret, &suite_id, b"key", &key_schedule_context, &mut key);

        // // Derive the base_nonce
        // let mut base_nonce = vec![0u8; Nn];
        // labeled_expand::<Kdf>(&secret, &suite_id, b"base_nonce", &key_schedule_context, &mut base_nonce);

        // // Derive the exporter_secret
        // let mut exporter_secret = vec![0u8; Nh];
        // labeled_expand::<Kdf>(&secret, &suite_id, b"exp", &key_schedule_context, &mut exporter_secret);

        // In KeySchedule(),
        //   secret = LabeledExtract(shared_secret, "secret", psk)
        //   key = LabeledExpand(secret, "key", key_schedule_context, Nk)
        //   base_nonce = LabeledExpand(secret, "base_nonce", key_schedule_context, Nn)
        //   exporter_secret = LabeledExpand(secret, "exp", key_schedule_context, Nh)
        // Instead of `secret` we derive an HKDF context which we run .expand() on to derive the
        // key-nonce pair.
        let (secret, secret_ctx) =
        labeled_extract::<Kdf>(&shared_secret.0, &suite_id, b"secret", &[mode]);

        // Empty fixed-size buffers
        let mut key = crate::aead::AeadKey::<A>::default();
        let mut base_nonce = crate::aead::AeadNonce::<A>::default();
        let mut exporter_secret = <ExporterSecret<Kdf> as Default>::default();

        // Fill the key, base nonce, and exporter secret. This only errors if the output values are
        // 255x the digest size of the hash function. Since these values are fixed at compile time, we
        // don't worry about it.
        secret_ctx
            .labeled_expand(&suite_id, b"key", &key_schedule_context, key.0.as_mut_slice())
            .expect("aead key len is way too big");
        secret_ctx
            .labeled_expand(
                &suite_id,
                b"base_nonce",
                &key_schedule_context,
                base_nonce.0.as_mut_slice(),
            )
            .expect("nonce len is way too big");
        secret_ctx
            .labeled_expand(
                &suite_id,
                b"exp",
                &key_schedule_context,
                exporter_secret.0.as_mut_slice(),
            )
            .expect("exporter secret len is way too big");

        // generate the export test vectors
        let mut exports = Vec::new();
        for &context in EXPORTER_CONTEXTS.iter() {
            let mut exported_value = vec![0u8; EXPORT_LEN];
            aead_ctx_r.export(context, &mut exported_value).unwrap();
            exports.push(ExporterTestVector {
                export_ctx: context.to_vec(),
                export_len: EXPORT_LEN,
                export_val: exported_value,
            });
        }

        // Produce encryptions
        let mut encryptions = Vec::new();
        let plaintexts = vec![
            b"4265617574792069732074727574682c20747275746820626561757479".to_vec(), // Example plaintext
        ];
        let aads = vec![
            b"436f756e742d323536".to_vec(), // Example AAD
        ];

        for (i, plaintext) in plaintexts.iter().enumerate() {
            let aad = &aads[i % aads.len()];
            let mut nonce = base_nonce.0.clone();
            nonce[nonce.len() - 1] ^= i as u8; // Simple nonce increment

            let ciphertext = aead_ctx_s.seal(plaintext, aad).unwrap();

            encryptions.push(EncryptionTestVector {
                plaintext: plaintext.clone(),
                aad: aad.clone(),
                _nonce: nonce.to_vec(),
                ciphertext,
            });
        }

        MainTestVector {
            aead_id: A::AEAD_ID,
            kdf_id: Kdf::KDF_ID,
            kem_id: Kem::KEM_ID,
            info: info.to_vec(),
            mode,
            ikm_recip: ikm_recip.to_vec(),
            ikm_sender: psk_extras.map(|e| e.ikm.to_vec()),
            sk_recip: sk_recip.to_bytes().to_vec(),
            sk_eph: sk_eph.to_bytes().to_vec(),
            sk_sender: psk_extras.map(|e| e.sender_keypair).map(|(sk, _)| sk.to_bytes().to_vec()),
            psk: psk_extras.map(|e| e.psk),
            psk_id: psk_extras.map(|e| e.psk_id),
            pk_recip: pk_recip.to_bytes().to_vec(),
            pk_sender: psk_extras.map(|e| e.sender_keypair).map(|(_, pk)| pk.to_bytes().to_vec()),
            _ikm_eph: ikm_eph.to_vec(),
            _pk_eph: pk_eph.to_bytes().to_vec(),
            encapped_key: encapped_key.to_bytes().to_vec(),
            shared_secret: shared_secret.0.to_vec(),
            _hpke_context: key_schedule_context.to_vec(),
            _key_schedule_secret: secret.to_vec(),
            _aead_key: key.0.to_vec(),
            _aead_base_nonce: base_nonce.0.to_vec(),
            _exporter_secret: exporter_secret.0.to_vec(),
            encryptions,
            exports,
        }
    }

    fn gen_ikm<Kem: TestableKem, R: CryptoRng + RngCore>(csprng: &mut R) -> GenericArray<u8, <Kem::PrivateKey as Serializable>::OutputSize> {
        let mut ikm: GenericArray<u8, <Kem::PrivateKey as Serializable>::OutputSize> =
            GenericArray::default();
        // Fill it with randomness
        csprng.fill_bytes(&mut ikm);
        ikm
    }

    /// Constructs an `OpModeR` from the given components. The variant constructed is determined solely
    /// by `mode_id`. This will panic if there is insufficient data to construct the variants specified
    /// by `mode_id`.
    fn make_op_mode_s<'a, Kem: KemTrait>(
        mode_id: u8,
        keypair: Option<(Kem::PrivateKey, Kem::PublicKey)>,
        psk: Option<&'a [u8]>,
        psk_id: Option<&'a [u8]>,
    ) -> OpModeS<'a, Kem> {
        // Deserialize the optional bundle
        let bundle = psk.map(|bytes| PskBundle {
            psk: bytes,
            psk_id: psk_id.unwrap(),
        });

        // These better be set if the mode ID calls for them
        match mode_id {
            0 => OpModeS::Base,
            1 => OpModeS::Psk(bundle.unwrap()),
            2 => OpModeS::Auth(keypair.unwrap()),
            3 => OpModeS::AuthPsk(keypair.unwrap(), bundle.unwrap()),
            _ => panic!("Invalid mode ID: {}", mode_id),
        }
    }

    // This macro takes in all the supported AEADs and dispatches the given test
    // vector to the test case with the appropriate types
    macro_rules! dispatch_testcase {
        // Step 1: Roll up the AEAD, KDF, and KEM types into tuples. We'll unroll them later
        ($tv:ident, ($( $aead_ty:ty ),*), ($( $kdf_ty:ty ),*), ($( $kem_ty:ty ),*)) => {
            dispatch_testcase!(@tup1 $tv, ($( $aead_ty ),*), ($( $kdf_ty ),*), ($( $kem_ty ),*))
        };
        // Step 2: Expand with respect to every AEAD
        (@tup1 $tv:ident, ($( $aead_ty:ty ),*), $kdf_tup:tt, $kem_tup:tt) => {
            $(
                dispatch_testcase!(@tup2 $tv, $aead_ty, $kdf_tup, $kem_tup);
            )*
        };
        // Step 3: Expand with respect to every KDF
        (@tup2 $tv:ident, $aead_ty:ty, ($( $kdf_ty:ty ),*), $kem_tup:tt) => {
            $(
                dispatch_testcase!(@tup3 $tv, $aead_ty, $kdf_ty, $kem_tup);
            )*
        };
        // Step 4: Expand with respect to every KEM
        (@tup3 $tv:ident, $aead_ty:ty, $kdf_ty:ty, ($( $kem_ty:ty ),*)) => {
            $(
                dispatch_testcase!(@base $tv, $aead_ty, $kdf_ty, $kem_ty);
            )*
        };
        // Step 5: Now that we're only dealing with 1 type of each kind, do the dispatch. If the test
        // vector matches the IDs of these types, run the test case.
        (@base $tv:ident, $aead_ty:ty, $kdf_ty:ty, $kem_ty:ty) => {
            if let (<$aead_ty>::AEAD_ID, <$kdf_ty>::KDF_ID, <$kem_ty>::KEM_ID) =
                ($tv.aead_id, $tv.kdf_id, $tv.kem_id)
            {
                println!(
                    "Running test case on {}, {}, {}",
                    stringify!($aead_ty),
                    stringify!($kdf_ty),
                    stringify!($kem_ty)
                );

                let tv = $tv.clone();
                test_case::<$aead_ty, $kdf_ty, $kem_ty>(tv);

                // This is so that code that comes after a dispatch_testcase! invocation will know that
                // the test vector matched no known ciphersuites
                continue;
            }
        };
    }

    #[test]
    fn gen_secp_test_vectors() {
        use serde_json::Value;
        use std::fs::File;

        dispatch_k256_testgen!(
            (AesGcm128, AesGcm256, ChaCha20Poly1305, ExportOnlyAead),
            (HkdfSha256),
            (DhK256HkdfSha256)
        );

        for tv in incomplete_test_vectors {
            // Extract the necessary fields from the JSON object
            //let key = hex::decode(tv["key"].as_str().expect("Missing key")).expect("Invalid key format");
            //let base_nonce = hex::decode(tv["base_nonce"].as_str().expect("Missing base_nonce")).expect("Invalid base_nonce format");
            let aad = b"example aad"; // Replace with actual AAD if available in the JSON
            let pt = b"4265617574792069732074727574682c20747275746820626561757479"; // Replace with actual plaintext if available in the JSON
            let export_ctx = b"example context"; // Replace with actual export context if available in the JSON
            let export_len = 32; // Replace with actual export length if available in the JSON

            gen_test_case::<AesGcm128, HkdfSha256, DhK256HkdfSha256>(tv);
            println!("ONE DONE!");
            // // Generate encryption test vectors
            // let encryption_test_vectors = generate_encryption_test_vectors::<A, Kdf, Kem>(&key, &base_nonce, aad, plaintext);
            // println!("{:?}", encryption_test_vectors);

            // // Generate exported values
            // let exported_values = generate_exported_values::<A, Kdf, Kem>(&key, &base_nonce, export_ctx, export_len);
            // println!("{:?}", exported_values);
        }
    }

    fn save_test_vectors_to_file(test_vectors: &[MainTestVector], file_path: &str) {
        let mut file = File::create(file_path).expect("Failed to create file");
        let json = serde_json::to_string_pretty(test_vectors).expect("Failed to serialize test vectors");
        file.write_all(json.as_bytes()).expect("Failed to write to file");
    }
    
    #[test]
    fn generate_and_save_test_vectors() {
        let test_vectors = generate_test_vectors();
        save_test_vectors_to_file(&test_vectors, "test-vectors.json");
    }
}

use sha2::{Digest, Sha256};

#[test]
fn generate_256bit_hash_output() {
    // Example input data
    let input_data = b"don't tread on my fursona";

    // Create a Sha256 object
    let mut hasher = Sha256::new();

    // Write input data
    hasher.update(input_data);

    // Read hash digest and consume hasher
    let result = hasher.finalize();

    // Print the 256-bit hash output
    println!("{:x}", result);
}

#[test]
fn kat_test() {
    let file = File::open("test-vectors-5f503c5.json").unwrap();
    let tvs: Vec<MainTestVector> = serde_json::from_reader(file).unwrap();

    for tv in tvs.into_iter() {
        // Ignore everything that doesn't use X25519, P256, P384 or P521, since that's all we support
        // right now
        if tv.kem_id != X25519HkdfSha256::KEM_ID
            && tv.kem_id != DhP256HkdfSha256::KEM_ID
            && tv.kem_id != DhP384HkdfSha384::KEM_ID
            && tv.kem_id != DhP521HkdfSha512::KEM_ID
            && tv.kem_id != DhK256HkdfSha256::KEM_ID
        {
            continue;
        }

        // This unrolls into 36 `if let` statements
        dispatch_testcase!(
            tv,
            (AesGcm128, AesGcm256, ChaCha20Poly1305, ExportOnlyAead),
            (HkdfSha256, HkdfSha384, HkdfSha512),
            (
                X25519HkdfSha256,
                DhP256HkdfSha256,
                DhP384HkdfSha384,
                DhP521HkdfSha512,
                DhK256HkdfSha256
            )
        );

        // The above macro has a `continue` in every branch. We only get to this line if it failed
        // to match every combination of the above primitives.
        panic!(
            "Unrecognized (AEAD ID, KDF ID, KEM ID) combo: ({}, {}, {})",
            tv.aead_id, tv.kdf_id, tv.kem_id
        );
    }
}
