#!/bin/bash

git clone --recursive -b 0.22.0 https://github.com/cloudflare/quiche
cd quiche
cargo build --package quiche --release --features ffi,pkg-config-meta,qlog
ln -s libquiche.so target/release/libquiche.so.0
mkdir quiche/deps/boringssl/src/lib
ln -vnf $(find target/release -name libcrypto.a -o -name libssl.a) quiche/deps/boringssl/src/lib/
cd ..
git clone https://github.com/curl/curl
cd curl
autoreconf -fi
./configure LDFLAGS="-Wl,-rpath,$PWD/../quiche/target/release" --with-openssl=$PWD/../quiche/quiche/deps/boringssl/src --with-quiche=$PWD/../quiche/target/release --with-nghttp2=/usr
make
sudo make install