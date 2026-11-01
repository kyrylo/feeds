#!/usr/bin/env python3
"""Synthetic local container acceptance and matched runtime measurements.

Requires the dedicated telesink-verification network/PostgreSQL container and
locally built telesink-{rust:verification,rails:reference} images. Never reads .env.
Run `native`, then `rails`, then `rollback`; results stay under tmp/rust-port.
"""
import base64
import html
import http.client
import http.cookiejar
import json
import os
from pathlib import Path
import re
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tmp/rust-port"
PREFIX = "telesink-verification"
SECRET = "0123456789abcdef" * 4
PASSWORD = "synthetic-verification-only"
PG = f"postgresql://telesink:{PASSWORD}@{PREFIX}-postgres:5432/"


def docker(*args, data=None):
    return subprocess.check_output(["docker", *args], input=data, text=True).strip()


def sql(database, statement):
    assert database in ("verification_native", "verification_rails", "postgres")
    return docker("exec", "-i", f"{PREFIX}-postgres", "psql", "-XAtq",
                  "-v", "ON_ERROR_STOP=1", "-U", "telesink", "-d", database,
                  data=statement)


def env_args(database, framework="rails"):
    values = {"DATABASE_URL": PG + database, "SECRET_KEY_BASE": SECRET,
              "POSTGRES_HOST": f"{PREFIX}-postgres", "POSTGRES_USER": "telesink",
              "POSTGRES_PASSWORD": PASSWORD, "RAILS_LOG_LEVEL": "warn"}
    for key in ("queue", "cache", "cable"):
        values[key.upper() + "_DATABASE_URL"] = PG + f"verification_{framework}_" + key
    return [part for key, value in values.items() for part in ("-e", f"{key}={value}")]


def wait_ready(base, path):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(base + path, timeout=3) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.HTTPError):
            pass
        time.sleep(0.5)
    raise RuntimeError("container did not become ready")


class Client:
    def __init__(self, base):
        self.base = base
        self.cookies = http.cookiejar.CookieJar()
        self.http = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.cookies))

    def get(self, path):
        return self.send(path)

    def send(self, path, data=None, content_type=None):
        headers = {} if content_type is None else {"Content-Type": content_type}
        req = urllib.request.Request(self.base + path, data=data, headers=headers)
        with self.http.open(req, timeout=30) as response:
            return response.status, response.read().decode()

    def login(self):
        _, page = self.get("/session/new")
        token = html.unescape(re.search(r'<meta name="csrf-token" content="([^"]+)"', page)[1])
        data = urllib.parse.urlencode({"email_address": "container@example.test",
                                      "password": "synthetic-password",
                                      "authenticity_token": token}).encode()
        code, page = self.send("/session", data, "application/x-www-form-urlencoded")
        assert code == 200 and "Container source" in page
        assert any(c.name == "session_id" for c in self.cookies)
        return page


class Cable:
    def __init__(self, client, page):
        address = urllib.parse.urlsplit(client.base)
        self.socket = socket.create_connection((address.hostname, address.port), timeout=10)
        self.socket.settimeout(10)
        key = base64.b64encode(os.urandom(16)).decode()
        cookies = "; ".join(f"{c.name}={c.value}" for c in client.cookies)
        request = (f"GET /cable HTTP/1.1\r\nHost: {address.netloc}\r\nOrigin: {client.base}\r\n"
                   f"Cookie: {cookies}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                   f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
                   "Sec-WebSocket-Protocol: actioncable-v1-json\r\n\r\n")
        self.socket.sendall(request.encode())
        response = bytearray()
        while not response.endswith(b"\r\n\r\n"):
            response += self.socket.recv(1)
        assert response.startswith(b"HTTP/1.1 101"), response
        tokens = list(dict.fromkeys(html.unescape(t) for t in re.findall(r'signed-stream-name="([^"]+)"', page)))
        assert tokens
        for token in tokens:
            identifier = json.dumps({"channel": "Turbo::StreamsChannel", "signed_stream_name": token})
            self.send({"command": "subscribe", "identifier": identifier})
        confirmed = 0
        while confirmed < len(tokens):
            message = self.read()
            assert message.get("type") != "reject_subscription", message
            confirmed += message.get("type") == "confirm_subscription"

    def exact(self, length):
        result = bytearray()
        while len(result) < length:
            chunk = self.socket.recv(length - len(result))
            if not chunk:
                raise RuntimeError("websocket closed")
            result += chunk
        return result

    def send(self, value):
        data = json.dumps(value).encode()
        mask = os.urandom(4)
        header = bytes([0x81, 0x80 | len(data)]) if len(data) < 126 else bytes([0x81, 0xFE]) + struct.pack("!H", len(data))
        self.socket.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def read(self):
        first, second = self.exact(2)
        assert first & 15 == 1, first
        length = second & 127
        if length == 126:
            length = struct.unpack("!H", self.exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self.exact(8))[0]
        return json.loads(self.exact(length))

    def until(self, needle):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if needle in str(self.read().get("message", "")):
                return
        raise RuntimeError("live event did not arrive")


class DockerHTTP(http.client.HTTPConnection):
    def connect(self):
        endpoint = json.loads(docker("context", "inspect"))[0]["Endpoints"]["docker"]["Host"]
        assert endpoint.startswith("unix://")
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(endpoint[7:])


def memory(name):
    connection = DockerHTTP("localhost", timeout=10)
    connection.request("GET", f"/containers/{name}/stats?stream=false&one-shot=true")
    response = connection.getresponse()
    assert response.status == 200
    result = json.load(response)
    connection.close()
    stats = result["memory_stats"]
    inactive = stats["stats"].get("inactive_file", stats["stats"].get("total_inactive_file", 0))
    return {"usage_bytes": stats["usage"], "inactive_file_bytes": inactive,
            "working_bytes": stats["usage"] - inactive,
            "anonymous_bytes": stats["stats"].get("anon", stats["stats"].get("total_rss")),
            "tasks": result.get("pids_stats", {}).get("current")}


def seed(database):
    fixture = json.loads((ROOT / "rust/tests/fixtures/rails-contracts.json").read_text())
    digest = fixture["session"]["password_digest"].replace("'", "''")
    assert sql(database, "SELECT count(*) FROM users") == "0", "requires an empty owned database"
    sql(database, f"""
INSERT INTO accounts (id,join_code,created_at,updated_at) VALUES (1,'container-join',now(),now());
INSERT INTO users (id,account_id,email_address,nickname,password_digest,role,created_at,updated_at)
VALUES (1,1,'container@example.test','Container','{digest}',2,now(),now());
INSERT INTO sinks (id,account_id,name,token,created_at,updated_at) VALUES (1,1,'Container source','container-token',now(),now());
INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES (1,1,now(),now());
INSERT INTO columns (sink_id,name,position,config,created_at,updated_at)
VALUES (1,'Retained column',1,'{{"filters":{{"event_types":["payment_succeeded"]}},"legacy_option":"keep"}}',now(),now());
INSERT INTO events (sink_id,event_type,text,search_text,properties,occurred_at,created_at,updated_at)
SELECT 1,'payment_succeeded','Seed event '||i,'payment_succeeded Seed event '||i||' amount '||i,
jsonb_build_object('amount',i),now()-(1000-i)*interval '1 minute',now(),now() FROM generate_series(1,1000) i;
SELECT setval('accounts_id_seq',1); SELECT setval('users_id_seq',1); SELECT setval('sinks_id_seq',1);
""")


def run(kind):
    native = kind == "native"
    database = f"verification_{kind}"
    name = f"{PREFIX}-{kind}"
    image = "telesink-rust:verification" if native else "telesink-rails:reference"
    port = 55440 if native else 55441
    options = ["--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges"] if native else ["-e", "SOLID_QUEUE_IN_PUMA=1"]
    docker("run", "-d", "--name", name, "--network", PREFIX, "--label", "telesink.verification=true",
           "-p", f"127.0.0.1:{port}:80", *env_args(database), *options, image)
    base = f"http://127.0.0.1:{port}"
    wait_ready(base, "/ready" if native else "/up")
    seed(database)
    client = Client(base)
    page = client.login()
    cable = Cable(client, page)
    payload = {"event": "payment_succeeded", "text": "Container live acceptance", "idempotency_key": "container-live", "properties": {"amount": 23}}
    assert client.send("/api/v1/sinks/container-token/events", json.dumps(payload).encode(), "application/json")[0] == 201
    cable.until("Container live acceptance")
    assert client.send("/api/v1/sinks/container-token/events", json.dumps(payload).encode(), "application/json")[0] == 201
    for i in range(50):
        payload.update(text=f"Container warm event {i}", idempotency_key=f"container-warm-{i}")
        assert client.send("/api/v1/sinks/container-token/events", json.dumps(payload).encode(), "application/json")[0] == 201
    cable.until("Container warm event 49")
    assert sql(database, "SELECT count(*) FROM events") == "1051"
    for i in range(100):
        path = ["/sinks/1", "/sinks/1?q=Seed", "/sinks/1?property_key=amount&property_op=gt&property_value=900", "/settings/nickname/edit"][i % 4]
        assert client.get(path)[0] == 200
    print(f"{kind}: 100 page reads, 52 API requests, live subscription verified", flush=True)
    time.sleep(5)
    samples = []
    for _ in range(10):
        samples.append(memory(name))
        time.sleep(1)
    cable.socket.close()
    running_image = json.loads(docker("inspect", name))[0]["Image"]
    result = {"kind": kind, "image": json.loads(docker("image", "inspect", running_image))[0],
              "memory": samples, "postgres_memory": memory(f"{PREFIX}-postgres"),
              "processes": docker("top", name, "-eo", "pid,ppid,rss,comm,args"),
              "http_reads": 100, "api_posts": 52, "events": 1051, "live_verified": True}
    # Image metadata contains only synthetic build configuration; no runtime env.
    (OUT / f"container-{kind}.json").write_text(json.dumps(result, indent=2) + "\n")
    if native:
        (OUT / "container-native-session.json").write_text(json.dumps({c.name: c.value for c in client.cookies}))
        docker("exec", name, "/telesink", "healthcheck")
        docker("exec", name, "/telesink", "db", "check")
        start = time.monotonic()
        docker("stop", "--time", "10", name)
        stopped = json.loads(docker("inspect", name))[0]["State"]
        assert stopped["ExitCode"] == 0 and not stopped["OOMKilled"], stopped
        docker("start", name)
        wait_ready(base, "/ready")
        assert "Container source" in client.get("/sinks/1")[1]
        assert sql(database, "SELECT count(*) FROM events") == "1051"
        result["restart_seconds"] = time.monotonic() - start
        result["session_and_data_survive_restart"] = True
        (OUT / f"container-{kind}.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"{kind}: results saved", flush=True)


def rollback():
    """Boot the unchanged production Rails image against native-created data."""
    docker("stop", "--time", "30", f"{PREFIX}-native", f"{PREFIX}-rails")
    name = f"{PREFIX}-rollback"
    before = sql("verification_native", "SELECT string_agg(version,',' ORDER BY version) FROM schema_migrations")
    docker("run", "-d", "--name", name, "--network", PREFIX, "--label", "telesink.verification=true",
           "-p", "127.0.0.1:55442:80", *env_args("verification_native", "rollback"),
           "-e", "SOLID_QUEUE_IN_PUMA=1", "telesink-rails:reference")
    base = "http://127.0.0.1:55442"
    wait_ready(base, "/up")
    client = Client(base)
    saved = json.loads((OUT / "container-native-session.json").read_text())
    client.http.addheaders = [("Cookie", "; ".join(f"{k}={v}" for k, v in saved.items()))]
    assert "Container source" in client.get("/sinks/1")[1], "Rails must accept the native session"
    assert "Retained column" in client.get("/sinks/1/columns/1/edit")[1]
    payload = {"text": "Rails rollback write", "event": "payment_succeeded",
               "idempotency_key": "container-rollback-rails", "properties": {"amount": 99}}
    assert client.send("/api/v1/sinks/container-token/events", json.dumps(payload).encode(), "application/json")[0] == 201
    deadline = time.monotonic() + 30
    while sql("verification_native", "SELECT count(*) FROM events") != "1052":
        assert time.monotonic() < deadline, "Rails worker did not persist the accepted event"
        time.sleep(0.5)
    assert sql("verification_native", "SELECT string_agg(version,',' ORDER BY version) FROM schema_migrations") == before
    docker("stop", "--time", "30", name)
    docker("start", f"{PREFIX}-native")
    client.base = "http://127.0.0.1:55440"
    wait_ready(client.base, "/ready")
    assert "Rails rollback write" in client.get("/sinks/1")[1]
    assert client.send("/api/v1/sinks/container-token/events", json.dumps(payload).encode(), "application/json")[0] == 201
    assert sql("verification_native", "SELECT count(*) FROM events") == "1052"
    assert sql("verification_native", "SELECT config->>'legacy_option' FROM columns WHERE id=1") == "keep"
    (OUT / "container-rollback.json").write_text(json.dumps({"rails_reads_native_data": True,
        "rails_accepts_native_session": True, "rails_worker_writes_retained_source": True,
        "native_reads_rails_write": True, "cross_runtime_idempotency": True,
        "migration_history_unchanged": True, "legacy_column_config_retained": True}, indent=2) + "\n")
    print("Rails/native rollback round trip passed", flush=True)


if __name__ == "__main__":
    assert sys.argv[1:] in (["native"], ["rails"], ["rollback"])
    if sys.argv[1] == "rollback":
        rollback()
    else:
        run(sys.argv[1])
