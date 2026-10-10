#!/usr/bin/env python3
"""Lists the rooms on the LiveKit server with their participant counts."""
from lkapi import call

status, reply = call("RoomService", "ListRooms", {})
for room in reply.get("rooms", []):
    print(room["name"], "participants", room.get("num_participants", 0), "publishers", room.get("num_publishers", 0),
          "recording", room.get("active_recording", False))
print("rooms:", len(reply.get("rooms", [])), "status", status)
