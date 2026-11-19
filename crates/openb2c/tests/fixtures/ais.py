"""Local-only AIS integration fixture; invoked by the Rust generation test.

Runs the exported application unchanged. The bank requires a locally signed client
certificate and a bearer token. No TLS checks are disabled and no live bank is used.
"""
import base64
import contextlib
import datetime
import http.server
import json
import os
from pathlib import Path
import sqlite3
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


class Bank(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, directory):
        self.pages = [[]]
        self.fail_page = None
        self.token = "local-ais-token"
        self.calls = []
        self.errors = []
        super().__init__(("127.0.0.1", 0), BankRequest)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(directory / "bank.crt", directory / "bank.key")
        tls.load_verify_locations(directory / "ca.crt")
        tls.verify_mode = ssl.CERT_REQUIRED
        self.socket = tls.wrap_socket(self.socket, server_side=True)

    @property
    def base(self):
        return f"https://127.0.0.1:{self.server_port}/open-banking/v4.0/aisp"


class BankRequest(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        parsed = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(parsed.query)
        page = int(query.get("page", ["1"])[0])
        self.server.calls.append((page, query))
        if (parsed.path != "/open-banking/v4.0/aisp/accounts/local-account/transactions"
                or query.get("fromBookingDateTime") != ["2025-01-01T00:00:00Z"]):
            self.server.errors.append(self.path)
            self.send_error(400)
            return
        if self.headers.get("Authorization") != f"Bearer {self.server.token}":
            self.send_error(401)
            return
        if self.server.fail_page == page:
            self.send_error(503)
            return
        pages = self.server.pages
        links = {}
        if page < len(pages):
            links["Next"] = parsed.path + "?" + urllib.parse.urlencode({
                "fromBookingDateTime": query["fromBookingDateTime"][0], "page": page + 1})
        data = json.dumps({"Data": {"Transaction": pages[page - 1]},
                           "Links": links, "Meta": {"TotalPages": len(pages)}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def certificates(directory):
    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=directory, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=30)
    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=Local AIS test CA", "-keyout", "ca.key", "-out", "ca.crt",
            "-addext", "basicConstraints=critical,CA:TRUE")
    for name, usage in [("bank", "serverAuth"), ("client", "clientAuth")]:
        openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN={name}",
                "-keyout", f"{name}.key", "-out", f"{name}.csr")
        (directory / f"{name}.ext").write_text(
            f"subjectAltName=IP:127.0.0.1\nextendedKeyUsage={usage}\n"
            "basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\n")
        openssl("x509", "-req", "-in", f"{name}.csr", "-CA", "ca.crt", "-CAkey", "ca.key",
                "-CAcreateserial", "-days", "1", "-extfile", f"{name}.ext", "-out", f"{name}.crt")
    (directory / "client.pem").write_bytes(
        (directory / "client.crt").read_bytes() + (directory / "client.key").read_bytes())


class Application:
    def __init__(self, binary, directory, bank):
        self.binary = binary
        self.directory = directory
        self.env = os.environ | {
            "DB_PATH": str(directory / "app.sqlite"), "HOST": "127.0.0.1", "PORT": "0",
            "NO_PROXY": "*", "no_proxy": "*",
            "NODE_ENV": "development", "AUTH_ENABLED": "true", "OPENB2C_APPLY_FIXTURES": "true",
            "OPEN_BANKING_BASE_URL": bank.base, "BANK_ACCOUNT_ID": "local-account",
            "BANK_ACCOUNT_NAME": "Simulated publisher", "BANK_SORT_CODE": "000000",
            "BANK_ACCOUNT_NUMBER": "00000000", "BANK_SYNC_FROM": "2025-01-01T00:00:00Z",
            "OPEN_BANKING_ACCESS_TOKEN_FILE": str(directory / "token"),
            "OPEN_BANKING_CLIENT_IDENTITY_FILE": str(directory / "client.pem"),
            "OPEN_BANKING_CA_CERT_FILE": str(directory / "ca.crt"),
        }
        self.process = None
        self.log = None

    def operator(self):
        result = subprocess.run([self.binary, "--create-api-key", "operator@example.test",
                                 "payments.import,payments.reconcile,downloads.publish,billing.read,accounting.read"],
                                env=self.env, capture_output=True, check=True, timeout=30)
        return json.loads(result.stdout)["key"]

    def start(self):
        self.log = open(self.directory / "app.log", "w+")
        self.process = subprocess.Popen([self.binary], env=self.env, stdout=self.log, stderr=self.log)
        for _ in range(300):
            if self.process.poll() is not None:
                raise AssertionError((self.directory / "app.log").read_text())
            for line in (self.directory / "app.log").read_text().splitlines():
                with contextlib.suppress(ValueError):
                    record = json.loads(line)
                    if record.get("message") == "OpenB2C server listening":
                        self.base = "http://" + record["address"]
                        return
            time.sleep(0.1)
        raise AssertionError("Application startup timed out")

    def stop(self):
        if self.process:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
            self.process = None
        if self.log:
            self.log.close()
            self.log = None

    def request(self, method, path, token=None, body=None, expected=200, raw=False):
        headers = {}
        if token:
            headers["Authorization"] = "Bearer " + token
        if body is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.base + path, method=method, headers=headers,
                                         data=None if body is None else json.dumps(body).encode())
        # Ignore host proxy settings: every HTTP request is to this local process.
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        try:
            response = opener.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            data = response.read()
            assert response.status == expected, (method, path, response.status, data)
        return data if raw else json.loads(data)

    def count(self, table):
        assert table in {"bank_transaction", "payment_allocation", "invoice", "entitlement"}
        with sqlite3.connect(self.env["DB_PATH"]) as db:
            return db.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]


def transaction(payment, key, amount=None, reference=None, status="BOOK", direction="Credit"):
    minor = payment["amount_minor"] if amount is None else amount
    return {"AccountId": "local-account", "TransactionId": key,
            "TransactionReference": "FPID-" + key,
            "TransactionInformation": payment["bank_reference"] if reference is None else reference,
            "CreditDebitIndicator": direction, "Status": status,
            "TransactionMutability": "Immutable",
            "BookingDateTime": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "Amount": {"Amount": f"{minor // 100}.{minor % 100:02}", "Currency": "GBP"}}


def trial(app, bank):
    # The bank rejects a client that trusts its server but supplies no client identity.
    tls = ssl.create_default_context(cafile=app.directory / "ca.crt")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                                         urllib.request.HTTPSHandler(context=tls))
    try:
        opener.open(bank.base + "/accounts/local-account/transactions", timeout=5)
    except (urllib.error.URLError, ssl.SSLError, ConnectionError, OSError):
        pass
    else:
        raise AssertionError("AIS accepted a client without its certificate")
    assert bank.calls == [], "Unverified client reached the AIS handler"
    operator = app.operator()
    app.start()
    request = app.request
    reader = request("POST", "/auth/signup", body={"email": "reader@example.test", "name": "Reader",
                     "password": "local-test-password-long-enough"}, expected=201)["access_token"]
    stranger = request("POST", "/auth/signup", body={"email": "stranger@example.test", "name": "Stranger",
                       "password": "another-local-password-long-enough"}, expected=201)["access_token"]
    file = request("POST", "/b2c/downloads", operator, {
        "idempotency_key": "publication", "name": "book.txt", "media_type": "text/plain",
        "content_base64": base64.b64encode(b"Publisher integration test\n").decode(),
        "resources": ["digital-book"]})
    content = f'/b2c/downloads/{file["id"]}/content'
    request("GET", content, reader, expected=404)
    request("GET", content, stranger, expected=404)

    def payment(key, quantity=1):
        order = request("POST", "/b2c/orders", reader, {
            "idempotency_key": key, "items": [{"price_id": 1, "quantity": quantity}]})["order"]
        result = request("POST", f'/b2c/orders/{order["id"]}/payments', reader,
                         {"idempotency_key": key})
        assert "<svg" in result["instructions"]["qr_svg"]
        assert not result["instructions"]["simulated"]
        return order, result

    def sync(expected=200, token=operator):
        return request("POST", "/backoffice/transfers/sync", token,
                       {"changes_since": "2025-01-01T00:00:00Z"}, expected=expected)

    def state(p):
        return request("GET", f'/b2c/payments/{p["id"]}', reader)["status"]

    order, paid = payment("first")
    receipt = transaction(paid, "receipt")
    bank.pages = [[dict(receipt, TransactionId="pending", Status="PDNG")]]
    before = len(bank.calls)
    sync(expected=401, token=None)
    sync(expected=403, token=reader)
    assert len(bank.calls) == before, "Unauthorised requests contacted AIS"
    sync()
    assert state(paid) == "pending"
    assert app.count("bank_transaction") == 0

    # Trust is explicit: the same valid identity cannot connect without the test CA.
    app.stop()
    del app.env["OPEN_BANKING_CA_CERT_FILE"]
    app.start()
    bank.pages = [[receipt]]
    sync(expected=503)
    assert app.count("bank_transaction") == 0
    app.stop()
    app.env["OPEN_BANKING_CA_CERT_FILE"] = str(app.directory / "ca.crt")
    app.start()

    # First page commits real evidence and fulfilment even if a later page fails.
    bank.pages = [[receipt], []]
    bank.fail_page = 2
    sync(expected=503)
    assert state(paid) == "succeeded"
    assert request("GET", content, reader, raw=True) == b"Publisher integration test\n"
    request("GET", content, stranger, expected=404)
    counts = tuple(app.count(t) for t in ("bank_transaction", "payment_allocation", "invoice", "entitlement"))
    bank.fail_page = None
    sync()
    assert tuple(app.count(t) for t in ("bank_transaction", "payment_allocation", "invoice", "entitlement")) == counts

    # Persisted credentials, receipts and allocations survive an actual process restart.
    app.stop()
    app.start()
    sync()
    assert state(paid) == "succeeded"
    assert request("GET", content, reader, raw=True) == b"Publisher integration test\n"
    assert tuple(app.count(t) for t in ("bank_transaction", "payment_allocation", "invoice", "entitlement")) == counts

    # Rotated credentials are reread; failure cannot fabricate or remove facts.
    (app.directory / "token").write_text("expired-token")
    sync(expected=503)
    assert app.count("bank_transaction") == 1
    bank.token = "renewed-local-token"
    (app.directory / "token").write_text(bank.token)
    sync()

    # An immutable identifier cannot silently change the amount of money received.
    bank.pages = [[dict(receipt, Amount={"Amount": "16.00", "Currency": "GBP"})]]
    sync(expected=409)
    assert app.count("bank_transaction") == 1
    assert state(paid) == "succeeded"

    # A confirmed outgoing fact is required to record a refund and revoke access.
    bank.pages = [[receipt], [transaction(paid, "refund", direction="Debit")]]
    sync()
    outgoing = next(t for t in request("GET", "/backoffice/transfers", operator)["items"]
                    if t["transaction_id"] == "refund")
    assert state(paid) == "succeeded"
    request("POST", f'/backoffice/transfers/{outgoing["id"]}/allocate', operator,
            {"payment_id": paid["id"], "amount_minor": paid["amount_minor"]})
    assert state(paid) == "refunded"
    request("GET", content, reader, expected=404)
    sync()
    assert state(paid) == "refunded"
    today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    report = request("POST", "/backoffice/reports/financial", operator,
                     {"from": "2025-01-01", "through": today})
    assert report["balanced"]
    bank_balance = next(row for row in report["trial_balance"] if row["code"] == "bank")
    assert bank_balance["balance_minor"] == 0

    # Partial and unmatched receipts stay visible until an authorised allocation.
    _, partial = payment("partial")
    first = transaction(partial, "partial-1", amount=700, reference="UNKNOWN")
    second = transaction(partial, "partial-2", amount=800)
    first["BookingDateTime"] = "2025-01-01T12:00:00Z"
    second["BookingDateTime"] = "2025-01-02T12:00:00Z"
    bank.pages = [[receipt, first], [second]]
    sync()
    assert state(partial) == "pending"
    transfers = request("GET", "/backoffice/transfers", operator)["items"]
    for key, amount in [("partial-1", 700), ("partial-2", 800)]:
        row = next(t for t in transfers if t["transaction_id"] == key)
        allocation = {"payment_id": partial["id"], "amount_minor": amount}
        path = f'/backoffice/transfers/{row["id"]}/allocate'
        request("POST", path, reader, allocation, expected=403)
        request("POST", path, operator, allocation)
        day = "2025-01-01" if key == "partial-1" else "2025-01-02"
        report = request("POST", "/backoffice/reports/financial", operator,
                         {"from": "2025-01-01", "through": day})
        assert report["balanced"]
        bank_balance = next(row for row in report["trial_balance"] if row["code"] == "bank")
        assert bank_balance["balance_minor"] == (700 if key == "partial-1" else 1500)
        deposits = next(row for row in report["trial_balance"] if row["code"] == "customer_deposits")
        assert deposits["balance_minor"] == (-700 if key == "partial-1" else 0)
    assert state(partial) == "succeeded"
    assert request("GET", "/backoffice/transfers", operator)["unresolved_count"] == 0
    outgoing = transaction(partial, "partial-refund", direction="Debit")
    outgoing["BookingDateTime"] = "2025-01-03T12:00:00Z"
    bank.pages = [[outgoing]]
    sync()
    row = next(t for t in request("GET", "/backoffice/transfers", operator)["items"]
               if t["transaction_id"] == "partial-refund")
    request("POST", f'/backoffice/transfers/{row["id"]}/allocate', operator,
            {"payment_id": partial["id"], "amount_minor": 1500})
    assert state(partial) == "refunded"
    request("GET", content, reader, expected=404)

    # The application-owned Nix override rejects fulfilment after access was attempted.
    _, rejected = payment("rejected", quantity=2)
    bank.pages = [[transaction(rejected, "rejected")]]
    sync()
    assert state(rejected) == "pending"
    row = next(t for t in request("GET", "/backoffice/transfers", operator)["items"]
               if t["transaction_id"] == "rejected")
    assert row["unallocated_minor"] == 3000 and row["review_reason"]
    assert row["allocations"] == []
    with sqlite3.connect(app.env["DB_PATH"]) as db:
        assert db.execute("SELECT count(*) FROM entitlement WHERE resource='rollback'").fetchone()[0] == 0
    sync()
    assert state(rejected) == "pending"

    # Cancelled orders retain late money without granting fulfilment.
    cancelled_order, cancelled = payment("cancelled")
    request("POST", f'/b2c/orders/{cancelled_order["id"]}/cancel', reader, {})
    bank.pages = [[transaction(cancelled, "late")]]
    sync()
    assert state(cancelled) != "succeeded"

    # A subscription activates from a bank receipt, not the creation request.
    sub = request("POST", "/b2c/subscriptions", reader,
                  {"idempotency_key": "membership", "price_id": 2})
    sid = sub["subscription"]["id"]
    assert sub["subscription"]["status"] == "pending"
    bank.pages = [[transaction(sub["payment"], "membership")]]
    sync()
    assert request("GET", f"/b2c/subscriptions/{sid}", reader)["subscription"]["status"] == "active"
    request("POST", f"/b2c/subscriptions/{sid}/cancel", reader, {"at_period_end": False})
    assert request("GET", f"/b2c/subscriptions/{sid}", reader)["subscription"]["status"] == "cancelled"
    # Leave one paid membership for the generated clock-boundary probe. This avoids
    # sleeps, editing dates in SQLite or adding a deployment clock override.
    renewal = request("POST", "/b2c/subscriptions", stranger,
                      {"idempotency_key": "renewable-membership", "price_id": 2})
    membership = request("POST", "/b2c/downloads", operator, {
        "idempotency_key": "membership-content", "name": "membership.txt", "media_type": "text/plain",
        "content_base64": base64.b64encode(b"Membership publication").decode(),
        "resources": ["reader-membership"]})
    bank.pages = [[transaction(renewal["payment"], "renewable-membership")]]
    sync()
    request("GET", f'/b2c/downloads/{membership["id"]}/content', stranger, raw=True)
    (app.directory / "trial-state.json").write_text(json.dumps({
        "subscription_id": renewal["subscription"]["id"], "file_id": membership["id"],
        "bank_reference": renewal["payment"]["bank_reference"]}))
    assert not bank.errors, bank.errors
    assert any(page == 2 for page, _ in bank.calls)
    print("AIS publisher REST trial passed: mTLS, auth, pagination, durable interruption, replay, restart, "
          "credential rotation, TLS rejection, conflicts, downloads, refunds, balanced journals, manual allocation, rejection rollback, cancellation, subscriptions.")


def main():
    os.umask(0o077)
    binary, directory = sys.argv[1], Path(sys.argv[2])
    certificates(directory)
    bank = Bank(directory)
    (directory / "token").write_text(bank.token)
    worker = threading.Thread(target=bank.serve_forever, daemon=True)
    worker.start()
    app = Application(binary, directory, bank)
    try:
        trial(app, bank)
    finally:
        app.stop()
        bank.shutdown()
        bank.server_close()
        worker.join(timeout=5)


if __name__ == "__main__":
    main()
