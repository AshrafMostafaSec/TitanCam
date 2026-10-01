"""Private bounded client for the receiver; safe to call from worker threads."""
import json
import os
from pathlib import Path
import socket
import uuid


def request(command, body=None):
    runtime = Path(os.environ.get('XDG_RUNTIME_DIR', Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local/state')) / 'titancam'))
    path = runtime / 'titancam/control.sock'
    message_id = uuid.uuid4().hex
    payload = json.dumps({'version': 1, 'id': message_id, 'command': command, 'body': body or {}}, separators=(',', ':')).encode() + b'\n'
    if len(payload) > 65536:
        raise ValueError('Control request too large')
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(18)
        connection.connect(str(path))
        connection.sendall(payload)
        data = bytearray()
        while b'\n' not in data:
            part = connection.recv(min(4096, 65537 - len(data)))
            if not part or len(data) + len(part) > 65536:
                raise ValueError('Unfinished or oversized control response')
            data.extend(part)
        response = json.loads(data)
    if response.get('version') != 1 or response.get('id') != message_id:
        raise ValueError('Control response mismatch')
    if not response.get('ok'):
        raise ValueError(response.get('error') or 'Receiver rejected command')
    return response['body']
