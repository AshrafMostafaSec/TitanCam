#include <usbmuxd.h>
#include <string.h>
int titan_usb_devices(char *out, int capacity) {
 usbmuxd_device_info_t *list=0; int count=usbmuxd_get_device_list(&list); if(count<0)return count;
 int used=0; for(int i=0;i<count;i++){if(list[i].conn_type!=CONNECTION_TYPE_USB)continue;size_t n=strnlen(list[i].udid,44);if(used+(int)n+1>=capacity)break;memcpy(out+used,list[i].udid,n);used+=n;out[used++]='\n';}
 if(list){usbmuxd_device_list_free(&list);}
 return used;
}
int titan_usb_connect(const char *udid,unsigned short port){usbmuxd_device_info_t d;int r=usbmuxd_get_device_by_udid(udid,&d);return r==1?usbmuxd_connect(d.handle,port):-1;}
