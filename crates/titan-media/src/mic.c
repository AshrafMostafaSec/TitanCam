#define _POSIX_C_SOURCE 200809L
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#define FRAMES 8192
struct tc_mic{struct pw_thread_loop *loop;struct pw_stream *stream;struct spa_hook listener;uint32_t channels;int16_t *samples;_Atomic uint64_t write,read,underruns;};
static void process(void *data){
 struct tc_mic*m=data;struct pw_buffer*b=pw_stream_dequeue_buffer(m->stream);if(!b)return;
 struct spa_buffer*buf=b->buffer;if(buf->n_datas==0||!buf->datas[0].data){pw_stream_queue_buffer(m->stream,b);return;}
 const uint32_t stride=m->channels*sizeof(int16_t);uint32_t n=b->requested?b->requested:256;
 if(n>buf->datas[0].maxsize/stride)n=buf->datas[0].maxsize/stride;
 int16_t*out=buf->datas[0].data;uint64_t rd=atomic_load_explicit(&m->read,memory_order_relaxed),wr=atomic_load_explicit(&m->write,memory_order_acquire);uint32_t available=wr-rd;if(available<n)atomic_fetch_add_explicit(&m->underruns,1,memory_order_relaxed);
 for(uint32_t i=0;i<n;i++){for(uint32_t c=0;c<m->channels;c++)out[i*m->channels+c]=i<available?m->samples[((rd+i)%FRAMES)*m->channels+c]:0;}
 atomic_store_explicit(&m->read,rd+(available<n?available:n),memory_order_release);
 buf->datas[0].chunk->offset=0;buf->datas[0].chunk->stride=stride;buf->datas[0].chunk->size=n*stride;b->size=n;pw_stream_queue_buffer(m->stream,b);
}
static const struct pw_stream_events events={PW_VERSION_STREAM_EVENTS,.process=process};
void *tc_mic_create(uint32_t channels){
 if(channels<1||channels>2){return NULL;}
 pw_init(NULL,NULL);struct tc_mic*m=calloc(1,sizeof(*m));if(!m)return NULL;m->channels=channels;m->samples=calloc(FRAMES*channels,sizeof(int16_t));m->loop=pw_thread_loop_new("titancam-audio",NULL);
 if(!m->samples||!m->loop)goto fail;
 m->stream=pw_stream_new_simple(pw_thread_loop_get_loop(m->loop),"TitanCam Microphone",pw_properties_new(PW_KEY_MEDIA_TYPE,"Audio",PW_KEY_MEDIA_CATEGORY,"Capture",PW_KEY_MEDIA_ROLE,"Communication",PW_KEY_MEDIA_CLASS,"Audio/Source",PW_KEY_NODE_NAME,"titancam_mic",PW_KEY_NODE_DESCRIPTION,"TitanCam Microphone",PW_KEY_NODE_LATENCY,"256/48000","node.always-process","true","node.want-driver","true",NULL),&events,m);
 if(!m->stream)goto fail;
 uint8_t buffer[1024];struct spa_pod_builder builder=SPA_POD_BUILDER_INIT(buffer,sizeof(buffer));struct spa_audio_info_raw info={.format=SPA_AUDIO_FORMAT_S16_LE,.rate=48000,.channels=channels};info.position[0]=channels==1?SPA_AUDIO_CHANNEL_MONO:SPA_AUDIO_CHANNEL_FL;if(channels==2)info.position[1]=SPA_AUDIO_CHANNEL_FR;
 const struct spa_pod *params[1]={spa_format_audio_raw_build(&builder,SPA_PARAM_EnumFormat,&info)};
 if(pw_stream_connect(m->stream,PW_DIRECTION_OUTPUT,PW_ID_ANY,PW_STREAM_FLAG_MAP_BUFFERS|PW_STREAM_FLAG_RT_PROCESS,params,1)<0)goto fail;
 if(pw_thread_loop_start(m->loop)<0){goto fail;}
 return m;
 fail: if(m->stream)pw_stream_destroy(m->stream);if(m->loop)pw_thread_loop_destroy(m->loop);free(m->samples);free(m);return NULL;
}
// Single producer / single consumer. Producer never overwrites samples being read.
uint32_t tc_mic_push(void *ptr,const int16_t *samples,uint32_t frames){struct tc_mic*m=ptr;uint64_t wr=atomic_load_explicit(&m->write,memory_order_relaxed),rd=atomic_load_explicit(&m->read,memory_order_acquire);if(frames>FRAMES-(wr-rd))return 0;for(uint32_t i=0;i<frames;i++)memcpy(m->samples+((wr+i)%FRAMES)*m->channels,samples+i*m->channels,m->channels*sizeof(int16_t));atomic_store_explicit(&m->write,wr+frames,memory_order_release);return frames;}
uint32_t tc_mic_queued(void *ptr){struct tc_mic*m=ptr;return atomic_load_explicit(&m->write,memory_order_acquire)-atomic_load_explicit(&m->read,memory_order_acquire);}
uint64_t tc_mic_underruns(void *ptr){struct tc_mic*m=ptr;return atomic_load_explicit(&m->underruns,memory_order_relaxed);}
void tc_mic_destroy(void *ptr){struct tc_mic*m=ptr;if(!m)return;pw_thread_loop_stop(m->loop);pw_stream_destroy(m->stream);pw_thread_loop_destroy(m->loop);free(m->samples);free(m);}
