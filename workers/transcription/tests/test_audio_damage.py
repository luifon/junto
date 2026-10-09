"""Decoding that stops early and gaps between packet timestamps."""

from __future__ import annotations

from collections.abc import Iterator
from fractions import Fraction
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import av
import av.error
import numpy as np
import pytest

from junto_transcribe.audio import AvAudioReader, decode_container
from junto_transcribe.engines import AudioDamagedError, AudioDecodeError, Samples

RATE = 16000
INVALID = av.error.InvalidDataError(1, "invalid data")


def frame(start: float | None, seconds: float = 1.0, level: float = 0.25, time_base: int = RATE) -> av.AudioFrame:
    samples = np.full((1, round(seconds * RATE)), int(level * 32767), dtype=np.int16)
    made = av.AudioFrame.from_ndarray(samples, format="s16", layout="mono")
    made.sample_rate = RATE
    made.time_base = Fraction(1, time_base)
    made.pts = None if start is None else round(start * time_base)
    return made


class Packet:
    def __init__(self, *content: av.AudioFrame | Exception) -> None:
        self.content = content

    def decode(self) -> list[av.AudioFrame]:
        for item in self.content:
            if isinstance(item, Exception):
                raise item
        return [item for item in self.content if isinstance(item, av.AudioFrame)]


def container(*packets: Packet | Exception, audio: bool = True) -> Any:
    def demux(stream: object) -> Iterator[Packet]:
        for item in packets:
            if isinstance(item, Exception):
                raise item
            yield item

    return SimpleNamespace(streams=SimpleNamespace(audio=[object()] if audio else []), demux=demux)


def decode(source: Any, max_gap_seconds: float = 60.0, block_seconds: float = 100.0) -> tuple[Samples, str | None]:
    """Everything that was decoded, and the damage code when decoding stopped early."""
    blocks: list[Samples] = [np.zeros(0, dtype=np.float32)]
    damage = None
    try:
        for block, _ in decode_container(source, round(block_seconds * RATE), round(max_gap_seconds * RATE)):
            blocks.append(block)
    except AudioDamagedError as error:
        damage = error.code
    return np.concatenate(blocks), damage


def loud(samples: Samples) -> list[tuple[float, float]]:
    """Ranges, in seconds, where the signal is not silent."""
    active = np.flatnonzero(np.abs(samples) > 0.1)
    if not active.size:
        return []
    breaks = np.flatnonzero(np.diff(active) > 1)
    starts = [active[0], *active[breaks + 1]]
    ends = [*active[breaks], active[-1]]
    return [(round(start / RATE, 2), round((end + 1) / RATE, 2)) for start, end in zip(starts, ends, strict=True)]


def test_valid_packets_are_decoded_whole() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(1), frame(2))))
    assert (len(samples), damage) == (3 * RATE, None)


def test_a_packet_that_cannot_be_decoded_after_valid_audio_is_reported_and_the_audio_before_it_is_kept() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(INVALID), Packet(frame(2))))
    assert damage == "decode_incomplete"
    assert loud(samples) == [(0.0, 1.0)]


def test_an_error_while_packets_are_read_is_reported_too() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(1)), INVALID))
    assert (len(samples), damage) == (2 * RATE, "decode_incomplete")


def test_blocks_before_the_error_are_delivered_before_the_error() -> None:
    delivered = []
    with pytest.raises(AudioDamagedError):
        for block, last in decode_container(container(Packet(frame(0, seconds=2.5)), Packet(INVALID)), RATE, 60 * RATE):
            delivered.append((len(block), last))
    assert delivered == [(RATE, False), (RATE, False), (RATE // 2, True)]


def test_a_file_from_which_nothing_can_be_decoded_fails() -> None:
    with pytest.raises(AudioDecodeError):
        decode(container(Packet(INVALID)))
    with pytest.raises(AudioDecodeError):
        decode(container(audio=False))
    with pytest.raises(AudioDecodeError):
        decode(container())


def test_a_gap_between_timestamps_becomes_silence_so_that_later_audio_keeps_its_time() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(10))))
    assert damage is None
    assert len(samples) == 11 * RATE
    assert loud(samples) == [(0.0, 1.0), (10.0, 11.0)]


def test_a_gap_is_also_kept_when_the_container_counts_in_milliseconds() -> None:
    samples, _ = decode(container(Packet(frame(0, time_base=1000)), Packet(frame(3.5, time_base=1000))))
    assert loud(samples) == [(0.0, 1.0), (3.5, 4.5)]


def test_a_gap_over_the_bound_stops_decoding_and_is_reported() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(100)), Packet(frame(101))), max_gap_seconds=60)
    assert damage == "timestamp_gap"
    assert len(samples) == RATE

    samples, damage = decode(container(Packet(frame(0)), Packet(frame(61))), max_gap_seconds=60)
    assert (len(samples), damage) == (62 * RATE, None)


def test_a_long_gap_is_delivered_in_blocks() -> None:
    lengths = [
        len(block)
        for block, _ in decode_container(container(Packet(frame(0)), Packet(frame(50))), 10 * RATE, 60 * RATE)
    ]
    assert lengths == [10 * RATE] * 5 + [RATE]


def test_timestamps_that_differ_by_a_few_milliseconds_insert_nothing() -> None:
    samples, _ = decode(container(Packet(frame(0)), Packet(frame(1.004)), Packet(frame(1.996)), Packet(frame(3.01))))
    assert len(samples) == 4 * RATE


def test_small_losses_are_restored_once_they_add_up() -> None:
    packets = [Packet(frame(index * 1.02)) for index in range(6)]
    samples, _ = decode(container(*packets))
    assert abs(len(samples) / RATE - 6.1) <= 0.06


def test_the_first_sample_is_the_start_of_the_file_whatever_its_timestamp() -> None:
    samples, damage = decode(container(Packet(frame(5)), Packet(frame(6))))
    assert (len(samples), damage) == (2 * RATE, None)


def test_packets_without_timestamps_follow_each_other() -> None:
    samples, damage = decode(container(Packet(frame(None)), Packet(frame(None))))
    assert (len(samples), damage) == (2 * RATE, None)


def test_a_timestamp_that_goes_back_is_reported_and_ends_the_audio() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(0.5)), Packet(frame(2))))
    assert (len(samples), damage) == (RATE, "timestamp_backward")

    samples, damage = decode(container(Packet(frame(0)), Packet(frame(1)), Packet(frame(-100))))
    assert (len(samples), damage) == (2 * RATE, "timestamp_backward")


def test_a_timestamp_that_goes_back_by_less_than_the_tolerance_is_accepted() -> None:
    samples, damage = decode(container(Packet(frame(0)), Packet(frame(0.96)), Packet(frame(1.99))))
    assert (len(samples), damage) == (3 * RATE, None)


def test_a_first_timestamp_below_zero_is_only_the_origin() -> None:
    samples, damage = decode(container(Packet(frame(-0.3)), Packet(frame(0.7))))
    assert (len(samples), damage) == (2 * RATE, None)


def test_a_file_with_more_than_one_audio_stream_is_refused() -> None:
    two = container(Packet(frame(0)))
    two.streams.audio = [object(), object()]
    with pytest.raises(AudioDecodeError) as refused:
        decode(two)
    assert refused.value.code == "not_one_audio_stream"
    with pytest.raises(AudioDecodeError) as refused:
        decode(container(audio=False))
    assert refused.value.code == "not_one_audio_stream"
    with pytest.raises(AudioDecodeError) as refused:
        decode(container(Packet(INVALID)))
    assert refused.value.code == "decode_failed"


def test_decoding_stops_at_the_sample_limit_with_one_frame_of_look_ahead() -> None:
    read = 0

    def packets() -> Iterator[Packet]:
        nonlocal read
        for second in range(3600):
            read += 1
            yield Packet(frame(second))

    source = container()
    source.demux = lambda stream: packets()
    blocks = []
    with pytest.raises(AudioDamagedError) as stopped:
        for block, last in decode_container(source, 1800 * RATE, 60 * RATE, max_samples=10 * RATE):
            blocks.append((len(block), last))
    assert stopped.value.code == "audio_limit"
    assert blocks == [(10 * RATE, True)]
    assert read <= 11


def test_audio_of_exactly_the_sample_limit_is_whole() -> None:
    source = container(*[Packet(frame(second)) for second in range(10)])
    blocks = [
        (len(block), last) for block, last in decode_container(source, 4 * RATE, 60 * RATE, max_samples=10 * RATE)
    ]
    assert blocks == [(4 * RATE, False), (4 * RATE, False), (2 * RATE, True)]


def test_inserted_silence_counts_against_the_sample_limit() -> None:
    source = container(Packet(frame(0)), Packet(frame(3000)), Packet(frame(3001)))
    blocks = []
    with pytest.raises(AudioDamagedError) as stopped:
        for block, _ in decode_container(source, 4 * RATE, 3600 * RATE, max_samples=10 * RATE):
            blocks.append(len(block))
    assert stopped.value.code == "audio_limit"
    assert sum(blocks) == 10 * RATE


def test_the_last_block_is_marked_also_when_the_audio_fills_it_exactly() -> None:
    source = container(*[Packet(frame(second)) for second in range(8)])
    blocks = [(len(block), last) for block, last in decode_container(source, 4 * RATE, 60 * RATE, max_samples=10**9)]
    assert blocks == [(4 * RATE, False), (4 * RATE, True)]


def test_decoding_stops_when_the_time_is_over() -> None:
    read = 0

    def packets() -> Iterator[Packet]:
        nonlocal read
        for second in range(100):
            read += 1
            yield Packet(frame(second))

    source = container()
    source.demux = lambda stream: packets()
    with pytest.raises(AudioDamagedError) as stopped:
        for _ in decode_container(source, 2 * RATE, 60 * RATE, max_samples=10**9, expired=lambda: read >= 5):
            pass
    assert stopped.value.code == "time_limit"
    assert read <= 6


@pytest.mark.skipif("matroska" not in av.formats_available or "flac" not in av.codecs_available, reason="no muxer")
def test_a_gap_in_a_real_file_is_kept(tmp_path: Path) -> None:
    path = tmp_path / "gap.mka"
    try:
        with av.open(str(path), "w", format="matroska") as output:
            stream = output.add_stream("flac", rate=RATE, layout="mono")
            packets = []
            for index in range(8):
                made = frame(index * 0.25, seconds=0.25)
                for packet in stream.encode(made):
                    packets.append(packet)
            packets += list(stream.encode(None))
            for packet in packets:
                if packet.pts is not None and packet.pts >= round(1.0 / packet.time_base):
                    packet.pts += round(9.0 / packet.time_base)
                    packet.dts = packet.pts
                output.mux(packet)
    except (av.error.FFmpegError, ValueError) as error:
        pytest.skip(f"this FFmpeg build cannot write the test file: {type(error).__name__}")
    with path.open("rb") as source:
        samples = np.concatenate([block for block, _ in AvAudioReader().blocks(source, 100 * RATE)])
    first, second = loud(samples)
    assert second[0] - first[1] == pytest.approx(9.0, abs=0.06)
    assert len(samples) / RATE == pytest.approx(11.0, abs=0.06)


@pytest.mark.skipif("matroska" not in av.formats_available or "flac" not in av.codecs_available, reason="no muxer")
def test_a_real_file_with_two_audio_streams_is_refused(tmp_path: Path) -> None:
    path = tmp_path / "two.mka"
    try:
        with av.open(str(path), "w", format="matroska") as output:
            streams = [output.add_stream("flac", rate=RATE, layout="mono") for _ in range(2)]
            for stream in streams:
                for packet in [*stream.encode(frame(0)), *stream.encode(None)]:
                    output.mux(packet)
    except (av.error.FFmpegError, ValueError) as error:
        pytest.skip(f"this FFmpeg build cannot write the test file: {type(error).__name__}")
    with path.open("rb") as source, pytest.raises(AudioDecodeError) as refused:
        list(AvAudioReader().blocks(source, 100 * RATE))
    assert refused.value.code == "not_one_audio_stream"
