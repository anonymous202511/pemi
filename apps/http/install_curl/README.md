## Fixing potential symbol lookup errors
After compiling and installing the quiche-enabled curl, you can verify the installation by running:
```bash
curl --version | grep quiche
```
This should output something containing `quiche`, for example:
```bash
curl 8.11.0-DEV (x86_64-pc-linux-gnu) libcurl/8.11.0-DEV BoringSSL zlib/1.2.11 brotli/1.0.9 libpsl/0.21.0 nghttp2/1.43.0 quiche/0.22.0 OpenLDAP/2.5.18
```

If your system has previously installed other versions of libcurl, you may encounter symbol lookup errors when running the new `curl`, such as:

```bash
curl: symbol lookup error: curl: undefined symbol: curl_easy_ssls_import
```

This issue can often be resolved by modifying the library search path of the `curl` to point to the directory where the newly compiled libcurl resides. For example:

```bash
sudo apt update && sudo apt install -y patchelf
sudo patchelf --set-rpath /usr/local/lib /usr/local/bin/curl
```