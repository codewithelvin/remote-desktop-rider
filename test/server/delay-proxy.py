import asyncio
import sys
import time

LISTEN_PORT, TARGET_PORT, ONE_WAY_MS = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
DELAY = ONE_WAY_MS / 1000


async def carry(reader, writer):
    queue = asyncio.Queue()

    async def deliver():
        while True:
            due, data = await queue.get()
            if data is None:
                writer.close()
                return
            wait = due - time.monotonic()
            if wait > 0:
                await asyncio.sleep(wait)
            writer.write(data)
            await writer.drain()

    sender = asyncio.create_task(deliver())
    while True:
        data = await reader.read(65536)
        await queue.put((time.monotonic() + DELAY, data or None))
        if not data:
            break
    await sender


async def handle(client_reader, client_writer):
    server_reader, server_writer = await asyncio.open_connection("127.0.0.1", TARGET_PORT)
    await asyncio.gather(carry(client_reader, server_writer), carry(server_reader, client_writer), return_exceptions=True)


async def main():
    server = await asyncio.start_server(handle, "127.0.0.1", LISTEN_PORT)
    async with server:
        await server.serve_forever()


asyncio.run(main())
