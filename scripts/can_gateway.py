#!/usr/bin/env python3
"""Receive CAN through python-can and stream bounded JSON lines on loopback.

No CAN transmit API is used. Adapter bitrate must match the existing vehicle bus.
Install scripts/can-requirements.txt into a virtualenv; see ECU_PROTOCOLS.md.
"""
import argparse
import asyncio
import json
import math
import time


def encode_frame(message, now=None):
    now = time.time() if now is None else now
    if (message.is_error_frame or message.is_remote_frame or message.is_fd
            or len(message.data) != 8 or not math.isfinite(message.timestamp)
            or abs(now - message.timestamp) > 2):
        return None
    limit = 0x1FFFFFFF if message.is_extended_id else 0x7FF
    if not 0 <= message.arbitration_id <= limit:
        return None
    return (json.dumps({
        'id': message.arbitration_id,
        'extended': message.is_extended_id,
        'timestampMs': round(message.timestamp * 1000),
        'data': list(message.data),
    }, separators=(',', ':')) + '\n').encode()


async def serve(bus, host, port):
    clients = set()

    async def connected(reader, writer):
        queue = asyncio.Queue(maxsize=64)
        clients.add(queue)
        try:
            while True:
                data = await asyncio.wait_for(queue.get(), timeout=30)
                writer.write(data)
                await asyncio.wait_for(writer.drain(), timeout=1)
        except (ConnectionError, asyncio.TimeoutError):
            pass
        finally:
            clients.discard(queue)
            writer.close()
            await writer.wait_closed()

    server = await asyncio.start_server(connected, host, port, limit=4096)
    async with server:
        while True:
            # recv timeout keeps shutdown responsive, without blocking the event loop.
            message = await asyncio.to_thread(bus.recv, 0.25)
            if message is None:
                continue
            data = encode_frame(message)
            if data is None:
                continue
            for queue in tuple(clients):
                if queue.full():
                    queue.get_nowait()  # Drop oldest, preserve current telemetry.
                queue.put_nowait(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--interface', required=True, help='socketcan, pcan, kvaser, slcan, ...')
    parser.add_argument('--channel', required=True, help='can0, PCAN_USBBUS1, COM3, /dev/tty.usbserial, ...')
    parser.add_argument('--bitrate', type=int, default=500000)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=29536)
    args = parser.parse_args()
    import can
    with can.Bus(interface=args.interface, channel=args.channel, bitrate=args.bitrate,
                 receive_own_messages=False, ignore_config=True) as bus:
        try:
            asyncio.run(serve(bus, args.host, args.port))
        except KeyboardInterrupt:
            pass


if __name__ == '__main__':
    main()
