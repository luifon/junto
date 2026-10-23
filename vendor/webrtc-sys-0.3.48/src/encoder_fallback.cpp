// Added to the vendored copy of webrtc-sys 0.3.48; this file is not in the published crate
// and contains no code of it.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#include "livekit/encoder_fallback.h"

#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "api/environment/environment_factory.h"
#include "api/video/i420_buffer.h"
#include "api/video/video_bitrate_allocation.h"
#include "api/video/video_frame.h"
#include "api/video_codecs/sdp_video_format.h"
#include "api/video_codecs/video_codec.h"
#include "api/video_codecs/video_encoder.h"
#include "api/video_codecs/video_encoder_factory.h"
#include "livekit/video_encoder_factory.h"
#include "modules/video_coding/include/video_codec_interface.h"
#include "modules/video_coding/include/video_error_codes.h"
#include "webrtc-sys/src/encoder_fallback.rs.h"

namespace livekit_ffi {
namespace {

constexpr int kWidth = 320;
constexpr int kHeight = 180;

// A hardware encoder that does not start, as VideoToolbox when it cannot make its session.
class FailingEncoder final : public webrtc::VideoEncoder {
 public:
  int InitEncode(const webrtc::VideoCodec*, const Settings&) override {
    return WEBRTC_VIDEO_CODEC_ERROR;
  }
  int32_t RegisterEncodeCompleteCallback(webrtc::EncodedImageCallback*) override {
    return WEBRTC_VIDEO_CODEC_OK;
  }
  int32_t Release() override { return WEBRTC_VIDEO_CODEC_OK; }
  int32_t Encode(const webrtc::VideoFrame&,
                 const std::vector<webrtc::VideoFrameType>*) override {
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  void SetRates(const RateControlParameters&) override {}
  EncoderInfo GetEncoderInfo() const override {
    EncoderInfo info;
    info.implementation_name = "failing hardware encoder";
    info.is_hardware_accelerated = true;
    return info;
  }
};

class FailingFactory final : public webrtc::VideoEncoderFactory {
 public:
  explicit FailingFactory(webrtc::SdpVideoFormat format) : format_(std::move(format)) {}
  std::vector<webrtc::SdpVideoFormat> GetSupportedFormats() const override {
    return {format_};
  }
  std::unique_ptr<webrtc::VideoEncoder> Create(const webrtc::Environment&,
                                               const webrtc::SdpVideoFormat&) override {
    return std::make_unique<FailingEncoder>();
  }

 private:
  webrtc::SdpVideoFormat format_;
};

class Collector final : public webrtc::EncodedImageCallback {
 public:
  Result OnEncodedImage(const webrtc::EncodedImage&,
                        const webrtc::CodecSpecificInfo* info) override {
    ++images;
    if (info) {
      codec = webrtc::CodecTypeToPayloadString(info->codecType);
    }
    return Result(Result::OK);
  }

  void OnFrameDropped(uint32_t, int, bool) override {}

  uint32_t images = 0;
  std::string codec;
};

}  // namespace

FallbackEncoding encode_with_a_failing_hardware_encoder(rust::Str codec) {
  const bool h264 = std::string(codec) == "H264";
  webrtc::SdpVideoFormat format =
      h264 ? webrtc::SdpVideoFormat("H264", {{"level-asymmetry-allowed", "1"},
                                             {"packetization-mode", "1"},
                                             {"profile-level-id", "42e01f"}})
           : webrtc::SdpVideoFormat("VP8");
  FailingFactory primary(format);
  // The format that the sender asks for when the backend VideoToolbox is chosen.
  webrtc::SdpVideoFormat requested = format;
  requested.parameters["x-livekit-video-encoder-backend"] = "videotoolbox";

  const webrtc::Environment env = webrtc::CreateEnvironment();
  std::unique_ptr<webrtc::VideoEncoder> encoder =
      CreateAdaptedEncoder(env, &primary, requested);

  webrtc::VideoCodec settings;
  settings.codecType = h264 ? webrtc::kVideoCodecH264 : webrtc::kVideoCodecVP8;
  settings.width = kWidth;
  settings.height = kHeight;
  settings.startBitrate = 300;
  settings.maxBitrate = 600;
  settings.minBitrate = 30;
  settings.maxFramerate = 30;
  settings.active = true;
  settings.qpMax = 56;
  settings.numberOfSimulcastStreams = 1;
  settings.simulcastStream[0].width = kWidth;
  settings.simulcastStream[0].height = kHeight;
  settings.simulcastStream[0].maxFramerate = 30;
  settings.simulcastStream[0].maxBitrate = 600;
  settings.simulcastStream[0].targetBitrate = 300;
  settings.simulcastStream[0].minBitrate = 30;
  settings.simulcastStream[0].qpMax = 56;
  settings.simulcastStream[0].active = true;
  if (h264) {
    *settings.H264() = webrtc::VideoEncoder::GetDefaultH264Settings();
  } else {
    *settings.VP8() = webrtc::VideoEncoder::GetDefaultVp8Settings();
  }

  FallbackEncoding result;
  result.init = encoder->InitEncode(
      &settings, webrtc::VideoEncoder::Settings(
                     webrtc::VideoEncoder::Capabilities(false), 1, 1200));
  Collector collector;
  encoder->RegisterEncodeCompleteCallback(&collector);
  webrtc::VideoBitrateAllocation allocation;
  allocation.SetBitrate(0, 0, 300000);
  encoder->SetRates(
      webrtc::VideoEncoder::RateControlParameters(allocation, 30.0));

  webrtc::scoped_refptr<webrtc::I420Buffer> buffer =
      webrtc::I420Buffer::Create(kWidth, kHeight);
  webrtc::I420Buffer::SetBlack(buffer.get());
  webrtc::VideoFrame frame = webrtc::VideoFrame::Builder()
                                 .set_video_frame_buffer(buffer)
                                 .set_rtp_timestamp(90000)
                                 .set_timestamp_us(1000)
                                 .build();
  std::vector<webrtc::VideoFrameType> types{webrtc::VideoFrameType::kVideoFrameKey};
  result.encode = encoder->Encode(frame, &types);
  result.images = collector.images;
  result.codec = rust::String(collector.codec);
  result.implementation =
      rust::String(encoder->GetEncoderInfo().implementation_name);
  encoder->Release();
  return result;
}

}  // namespace livekit_ffi
