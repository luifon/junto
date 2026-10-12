"""Only the test LiveKit container can be stopped and started by the live tests."""
import http.client
import http.server
import socket


class DockerConnection(http.client.HTTPConnection):
    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect('/var/run/docker.sock')


class Control(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        routes = {
            '/stop': '/containers/junto-linux1-livekit/stop?t=1',
            '/start': '/containers/junto-linux1-livekit/start',
        }
        if self.path not in routes:
            self.send_error(403)
            return
        connection = DockerConnection('localhost', timeout=30)
        try:
            connection.request('POST', routes[self.path], body=b'')
            response = connection.getresponse()
            body = response.read()
            self.send_response(response.status)
            self.end_headers()
            self.wfile.write(body)
        finally:
            connection.close()


http.server.HTTPServer(('0.0.0.0', 8080), Control).serve_forever()
