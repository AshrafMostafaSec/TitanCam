#include <opus/opus.h>
void titan_opus_bitrate(OpusEncoder *encoder, int bitrate) { opus_encoder_ctl(encoder, OPUS_SET_BITRATE(bitrate)); }
