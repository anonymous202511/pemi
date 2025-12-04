#!/bin/bash

usage () {
   echo "USAGE: $0 [all|0|1]"
	echo "0 = nginx"
   echo "1 = init"
	exit 1
}

if [ $# -ne 1 ]; then
	usage
fi

# only need to run once
init () {
   wget https://nginx.org/download/nginx-1.16.1.tar.gz
   tar xzvf nginx-1.16.1.tar.gz
   git clone --recursive https://github.com/anonymous202511/quiche-nginx.git
   mv quiche-nginx quiche
}

build_nginx () {
cd nginx-1.16.1
patch -N -r- -p01 < ../quiche/nginx/nginx-1.16.patch
cp ../ngx_http_v3_module* src/http/v3/
./configure                                 \
   --prefix=/etc/nginx                           \
   --build="quiche-$(git --git-dir=../quiche/.git rev-parse --short HEAD)" \
   --with-http_ssl_module                  \
   --with-http_v2_module                   \
   --with-http_v3_module                   \
   --with-openssl=../quiche/quiche/deps/boringssl \
   --with-quiche=../quiche
make -j$(nproc)
sudo ln -f -s $(pwd)/objs/nginx /usr/bin/nginx
}

if [ $1 == "all" ]; then
	init
   build_nginx
elif [ $1 -eq 0 ]; then
	build_nginx
elif [ $1 -eq 1 ]; then
   init
else
	usage
fi