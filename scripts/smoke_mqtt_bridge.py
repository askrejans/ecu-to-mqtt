#!/usr/bin/env python3
"""Exercise every ECU profile through the built bridge and a local MQTT broker.

Requires cargo build, mosquitto and mosquitto_sub on PATH. All fixtures, sockets
and broker processes are local and temporary; no vehicle hardware is contacted.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time


CAN_CASES = [
    ('megasquirt_can_dash', 1512, False, [3, 245, 23, 112, 7, 148, 2, 238]),
    ('megasquirt_can_realtime', 1520, False, [0, 0, 0, 0, 0, 0, 23, 112]),
    ('haltech_can_v2', 0x360, False, [23, 112, 3, 245, 2, 238, 0, 0]),
    ('maxxecu_can_v12', 0x520, False, [112, 23, 238, 2, 245, 3, 152, 3]),
    ('maxxecu_can_v13', 0x520, False, [112, 23, 238, 2, 245, 3, 152, 3]),
    ('ecumaster_emu_can', 0x600, False, [112, 23, 150, 25, 101, 0, 0, 0]),
    ('aemnet_can', 0x01F0A000, True, [60, 0, 0, 0, 128, 0, 25, 90]),
    ('link_generic_dash', 1000, False, [0, 0, 112, 23, 101, 0, 0, 0]),
    ('link_generic_dash2', 1000, False, [23, 112, 0, 100, 140, 75, 138, 140]),
    ('motec_m1_pdm', 0x118, False, [60, 75, 100, 90, 95, 30, 80, 85]),
]


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def exercise(binary, directory, broker_port, profile, can_case=None):
    stop_stream = threading.Event()
    errors = []
    topic = f'/g86-smoke/{profile}/telemetry'
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        listener.listen(1)
        listener.settimeout(10)

        def feed():
            try:
                with listener.accept()[0] as stream:
                    stream.settimeout(3)
                    while not stop_stream.is_set():
                        if can_case:
                            _, identifier, extended, data = can_case
                            packet = (json.dumps({'id': identifier, 'extended': extended,
                                'timestampMs': round(time.time() * 1000), 'data': data}) + '\n').encode()
                            stream.sendall(packet[:11])
                            stream.sendall(packet[11:])
                        else:
                            expected = b'A' if profile == 'speeduino' else b'a\x00\x06'
                            command = bytearray()
                            while len(command) < len(expected):
                                data = stream.recv(len(expected) - len(command))
                                if not data:
                                    return
                                command.extend(data)
                            if command != expected:
                                raise AssertionError(f'Unexpected ECU write: {command!r}')
                            if profile == 'speeduino':
                                packet = bytearray(138)
                                packet[6:11] = bytes([65, 130, 100, 138, 147])
                                packet[14:16] = (6000).to_bytes(2, 'little')
                                packet[25] = 75
                                stream.sendall(packet)
                                continue
                            packet = bytearray(112)
                            packet[6:8] = (6000).to_bytes(2, 'big')
                            packet[22:24] = (1940).to_bytes(2, 'big')
                            stream.sendall(packet[:37])
                            if stop_stream.wait(.18):
                                return
                            stream.sendall(packet[37:])
                        if stop_stream.wait(.05):
                            return
            except (ConnectionError, OSError) as error:
                if not stop_stream.is_set():
                    errors.append(error)
            except Exception as error:
                errors.append(error)

        feeder = threading.Thread(target=feed, daemon=True)
        feeder.start()
        config = directory / f'{profile}.toml'
        config.write_text(f'''ecu_protocol = "{profile}"
connection_type = "tcp"
tcp_host = "127.0.0.1"
tcp_port = {listener.getsockname()[1]}
mqtt_host = "127.0.0.1"
mqtt_port = {broker_port}
mqtt_base_topic = "/g86-smoke/{profile}/"
log_level = "warn"
''')
        subscriber = subprocess.Popen(['mosquitto_sub', '-h', '127.0.0.1', '-p', str(broker_port),
            '-t', topic, '-C', '1', '-W', '10'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        process = None
        try:
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith('ECU_TO_MQTT_')}
            process = subprocess.Popen([str(binary), '--config', str(config)], cwd=directory,
                env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            output, error = subscriber.communicate(timeout=12)
            if subscriber.returncode:
                raise AssertionError(f'{profile}: MQTT subscription failed: {error}')
            payload = json.loads(output)
            assert payload['schema'] == 1, payload
            assert abs(payload['channels']['rpm'] - 6000) < .2, payload
            assert abs(time.time() * 1000 - payload['timestampMs']) < 2000, payload
            assert payload.get('partial', False) == bool(can_case), payload
            assert payload['bootId'] and payload['sequence'] >= 0, payload
            assert payload['source'], payload
            print(f'PASS {profile}: ECU TCP → bridge → MQTT broker → subscriber', flush=True)
        finally:
            stop_stream.set()
            stop(process)
            stop(subscriber)
            feeder.join(timeout=4)
        if errors:
            raise errors[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/ecu-to-mqtt')
    args = parser.parse_args()
    for command in ('mosquitto', 'mosquitto_sub'):
        if not shutil.which(command):
            parser.error(f'{command} must be installed and on PATH')
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error('Build the bridge first with cargo build')
    with tempfile.TemporaryDirectory(prefix='g86-ecu-mqtt-smoke-') as temporary:
        directory = Path(temporary)
        port = free_port()
        config = directory / 'mosquitto.conf'
        config.write_text(f'listener {port} 127.0.0.1\nallow_anonymous true\npersistence false\n')
        broker = subprocess.Popen(['mosquitto', '-c', str(config)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for attempt in range(50):
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        break
                except OSError:
                    if broker.poll() is not None or attempt == 49:
                        raise RuntimeError('Local test broker did not start')
                    time.sleep(.05)
            for case in CAN_CASES:
                exercise(binary, directory, port, case[0], case)
            for profile in ('speeduino', 'ms2', 'ms3', 'ms3_pro', 'microsquirt'):
                exercise(binary, directory, port, profile)
        finally:
            stop(broker)


if __name__ == '__main__':
    main()
