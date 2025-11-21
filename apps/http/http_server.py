#!/usr/bin/env python3
"""
https://gist.github.com/mdonkers/63e115cc0c79b4f6b8b3a6b797e485c7
Very simple HTTP server in python for logging requests
Usage::
    ./server.py [<ip>]
"""
from http.server import BaseHTTPRequestHandler, HTTPServer
import logging
import os
from common import pemilog
import argparse


class S(BaseHTTPRequestHandler):
    def _set_response(self):
        self.send_response(200)
        self.send_header("Content-type", "text/html")
        self.end_headers()

    def do_GET(self):
        logging.info(
            "GET request,\nPath: %s\nHeaders:\n%s",
            str(self.path),
            str(self.headers).strip(),
        )
        self.send_response(200)
        # get file length
        file_length = os.path.getsize(self.path)
        self.send_header("Content-Length", str(file_length))
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header(
            "Content-Disposition",
            'attachment; filename="{}"'.format(os.path.basename(self.path)),
        )
        self.end_headers()
        pemilog(f"GET request for {self.path}, length {file_length}")
        try:
            if not os.path.exists(self.path):
                raise Exception(f"File not found: {self.path}")
            with open(self.path, "rb") as f:
                self.wfile.write(f.read())
        except Exception as e:
            print(str(e))

    def do_POST(self):
        content_length = int(
            self.headers["Content-Length"]
        )  # <--- Gets the size of data
        post_data = self.rfile.read(content_length)  # <--- Gets the data itself
        logging.debug(
            "POST request,\nPath: %s\nHeaders:\n%s\nBody: (%d bytes)",
            str(self.path),
            str(self.headers).strip(),
            len(post_data),
        )

        self._set_response()
        self.wfile.write("POST request for {}".format(self.path).encode("utf-8"))


def run(server_class=HTTPServer, handler_class=S, port=2222, ip=""):
    logging.basicConfig(level=logging.DEBUG)
    server_address = (ip, port)
    httpd = server_class(server_address, handler_class)
    logging.info("Starting httpd...")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    httpd.server_close()
    logging.info("Stopping httpd...")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="A http server used with nginx")

    parser.add_argument(
        "--ip",
        type=str,
        default="127.0.0.1",
        help="IP address of the server(default: 127.0.0.1)",
    )

    args = parser.parse_args()
    run(ip=args.ip)
