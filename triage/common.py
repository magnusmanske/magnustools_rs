"""Shared helpers for the Buggregator triage scripts.

The database is reached through the local SSH tunnel (port 3398) with the
credentials in `replica.my.cnf`; override the location with the environment
variables below. The password is passed in `MYSQL_PWD`, never on the command
line.
"""
import os
import subprocess

DEFAULT_DB = "s51079__buggregator"
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = "3398"


def _parse_cnf(path):
    user = password = None
    section = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line[0] in "#;":
                continue
            if line.startswith("[") and line.endswith("]"):
                section = line[1:-1]
                continue
            if section != "client" or "=" not in line:
                continue
            key, value = line.split("=", 1)
            key = key.strip()
            value = value.strip().strip("'\"")
            if key == "user":
                user = value
            elif key == "password":
                password = value
    if not user or password is None:
        raise SystemExit(f"{path}: no [client] user/password")
    return user, password


def mysql():
    """The mysql command and its environment."""
    host = os.environ.get("BUGGREGATOR_DB_HOST", DEFAULT_HOST)
    port = os.environ.get("BUGGREGATOR_DB_PORT", DEFAULT_PORT)
    database = os.environ.get("BUGGREGATOR_DB", DEFAULT_DB)
    cnf = os.environ.get("BUGGREGATOR_CNF", "replica.my.cnf")
    user, password = _parse_cnf(cnf)
    command = ["mysql", "--protocol=TCP", "--batch", "--raw", "--skip-column-names",
               "-h", host, "-P", port, "-u", user, database]
    return command, {**os.environ, "MYSQL_PWD": password}


def sql(query):
    """Run SQL, returning stdout."""
    command, env = mysql()
    result = subprocess.run(command, input=query, text=True, capture_output=True, env=env)
    if result.returncode:
        raise SystemExit(result.stderr.strip() or f"mysql failed ({result.returncode})")
    return result.stdout


def rows(query):
    """Run a query whose values contain no tabs or newlines."""
    return [line.split("\t") for line in sql(query).rstrip("\n").split("\n") if line]


def ids(values):
    return ",".join(str(v) for v in sorted(values))
