#!/bin/sh
# Makes the certificates of the TLS mode of the simulated HTTP server. Run it in this
# folder with OpenSSL 3. Each key is Ed25519, so each signature has one size.
set -eu
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT

# sign <name> <serial> <start> <end> <extensions> <ca options>...
sign() {
  name=$1 serial=$2 start=$3 end=$4 extensions=$5
  shift 5
  : > "$w/index.txt"
  echo "$serial" > "$w/serial"
  cat > "$w/ca.cnf" <<END
[ca]
default_ca = d
[d]
database = $w/index.txt
new_certs_dir = $w
serial = $w/serial
default_md = default
policy = p
unique_subject = no
[p]
commonName = supplied
[x]
$extensions
END
  openssl ca -batch -config "$w/ca.cnf" -extensions x -in "$w/$name.csr" \
    -out "$w/$name.full" -notext -startdate "$start" -enddate "$end" "$@"
  openssl x509 -in "$w/$name.full" -out "$name.pem"
}

# request <name> <common name>
request() {
  openssl genpkey -algorithm ed25519 -out "$1.key"
  openssl req -new -key "$1.key" -subj "/CN=$2" -out "$w/$1.csr"
}

CA="basicConstraints = critical, CA:true
keyUsage = critical, keyCertSign, cRLSign"

# leaf <name> <host> <serial> <start> <end> <ca>
leaf() {
  request "$1" "$2"
  sign "$1" "$3" "$4" "$5" "basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:$2" -cert "$6.pem" -keyfile "$6.key"
}

request ca "Foundation test CA"
sign ca 01 20250101000000Z 20360101000000Z "$CA" -selfsign -keyfile ca.key
request other-ca "Foundation other test CA"
sign other-ca 02 20250101000000Z 20360101000000Z "$CA" -selfsign -keyfile other-ca.key
leaf influx influx 10 20260101000000Z 20270101000000Z ca
leaf other other 11 20260101000000Z 20270101000000Z ca
leaf expired influx 12 20260101000000Z 20260201000000Z ca
leaf stranger influx 13 20260101000000Z 20270101000000Z other-ca
rm ca.key other-ca.key
