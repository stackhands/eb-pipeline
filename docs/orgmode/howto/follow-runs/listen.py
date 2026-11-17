# A webhook to try [notify] against: takes one POST on 127.0.0.1:8765,
# prints what it got, and exits; after a minute without one, it exits too.
import http.server
import json


class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        doc = json.loads(body)
        kind = self.headers["Content-Type"]
        print(f"POST {self.path} {kind}: {doc['kind']} of run {doc['run']}, {doc['status']}", flush=True)
        self.send_response(204)
        self.end_headers()

    def log_message(self, format, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 8765), Hook)
server.timeout = 60
print("listening on http://127.0.0.1:8765", flush=True)
server.handle_request()
