#!/usr/bin/env python3
"""Generate runner-only TLS fixtures; installation into OS trust is external.

The CA private key exists only in memory. Never use these certificates outside
an ephemeral acceptance runner. Existing output directories are never reused.
"""
import argparse
import datetime
import ipaddress
import os
import pathlib

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID


def write_new(path, content, mode):
    descriptor = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(content)
    # Explicit mode also handles a permissive umask. On Windows the runner's
    # isolated temporary-directory ACL remains the security boundary.
    path.chmod(mode)


def generate(directory):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise RuntimeError('TLS fixtures may be generated only on an ephemeral GitHub runner')
    directory = pathlib.Path(directory).absolute()
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    leaf_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    beginning = datetime.datetime.utcnow() - datetime.timedelta(minutes=1)
    expiration = beginning + datetime.timedelta(days=2)
    ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'NexusBro disposable route acceptance CA')])
    ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name)
          .public_key(ca_key.public_key()).serial_number(x509.random_serial_number())
          .not_valid_before(beginning).not_valid_after(expiration)
          .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .add_extension(x509.KeyUsage(digital_signature=False, content_commitment=False,
                                      key_encipherment=False, data_encipherment=False,
                                      key_agreement=False, key_cert_sign=True, crl_sign=True,
                                      encipher_only=False, decipher_only=False), critical=True)
          .add_extension(x509.SubjectKeyIdentifier.from_public_key(ca_key.public_key()), critical=False)
          .sign(ca_key, hashes.SHA256()))
    leaf_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
    leaf = (x509.CertificateBuilder().subject_name(leaf_name).issuer_name(ca.subject)
            .public_key(leaf_key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(beginning).not_valid_after(expiration)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.SubjectAlternativeName([
                x509.IPAddress(ipaddress.ip_address('127.0.0.1')), x509.DNSName('localhost')]), critical=False)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .add_extension(x509.KeyUsage(digital_signature=True, content_commitment=False,
                                        key_encipherment=True, data_encipherment=False,
                                        key_agreement=False, key_cert_sign=False, crl_sign=False,
                                        encipher_only=False, decipher_only=False), critical=True)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(ca_key.public_key()), critical=False)
            .sign(ca_key, hashes.SHA256()))
    write_new(directory / 'server.key', leaf_key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption()), 0o600)
    write_new(directory / 'server.crt', leaf.public_bytes(serialization.Encoding.PEM), 0o644)
    write_new(directory / 'ca.crt', ca.public_bytes(serialization.Encoding.PEM), 0o644)
    return directory


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', required=True, help='New output directory; must not exist')
    output = generate(parser.parse_args().out)
    for filename in ('ca.crt', 'server.crt', 'server.key'):
        print(output / filename)
