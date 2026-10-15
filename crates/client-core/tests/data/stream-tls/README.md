# Certificates of the TLS test of the event stream

The tests in `src/stream/socket.rs` read these files. They are for tests only.

- `authority.der`: a certificate authority, `CN=Junto test authority`, P-256. Its key was
  deleted after it signed `server.der`, so it cannot sign another certificate.
- `server.der`: the certificate of the server, `CN=127.0.0.1`, subject alternative name
  `IP:127.0.0.1`, extended key usage `serverAuth`, signed by the authority.
- `server-key.der`: the P-256 key of `server.der`, PKCS#8.

Both certificates are valid from 2026-10-15 to 2126-09-21. They were made with OpenSSL:

```sh
openssl ecparam -name prime256v1 -genkey -noout -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days 36500 -subj "/CN=Junto test authority" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" \
    -out ca.pem
openssl ecparam -name prime256v1 -genkey -noout -out leaf.key
openssl req -new -key leaf.key -subj "/CN=127.0.0.1" -out leaf.csr
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n' > ext.cnf
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -sha256 \
    -days 36500 -extfile ext.cnf -out leaf.pem
openssl x509 -in ca.pem -outform DER -out authority.der
openssl x509 -in leaf.pem -outform DER -out server.der
openssl pkcs8 -topk8 -nocrypt -in leaf.key -outform DER -out server-key.der
```
