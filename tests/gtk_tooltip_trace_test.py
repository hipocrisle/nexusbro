"""Contract test against a fake native GTK ABI; never starts GTK/a browser."""
from pathlib import Path
import json,os,subprocess,tempfile
ROOT=Path(__file__).resolve().parents[1]
LIB=r'''
#include <stdarg.h>
#include <string.h>
#include <stdlib.h>
static int a,b,has,sets,triggers,texts;
void *nb_test_widget(void){return &a;}
void *nb_test_other(void){return &b;}
void gtk_widget_set_has_tooltip(void*w,int n){if(w!=&a&&w!=&b)abort();has=n;sets++;}
void gtk_widget_trigger_tooltip_query(void*w){if(w!=&a)abort();triggers++;}
void gtk_tooltip_set_text(void*w,const char*t){if(w!=&a||strcmp(t,"PRIVATE_SENTINEL_URL"))abort();texts++;}
int gtk_widget_get_has_tooltip(void*w){if(w!=&a)abort();return has;}
void *gtk_widget_get_settings(void*w){if(w!=&a)abort();return &b;}
const char *g_type_name_from_instance(void*w){return w==&a?"WebKitWebView":"OtherWidget";}
void g_object_get(void*w,const char*p,...){if(w!=&b)abort();va_list v;va_start(v,p);while(p){void*out=va_arg(v,void*);if(!strcmp(p,"gtk-enable-tooltips"))*(int*)out=1;else if(!strcmp(p,"gtk-tooltip-timeout"))*(unsigned*)out=500;else if(!strcmp(p,"gtk-tooltip-browse-timeout"))*(unsigned*)out=60;else abort();p=va_arg(v,const char*);}va_end(v);}
int nb_test_ok(void){return sets==213&&triggers==2&&texts==1&&has==1;}
'''
MAIN=r'''
#include <unistd.h>
#include <fcntl.h>
#include <stdlib.h>
#include <stdio.h>
void *nb_test_widget(void);void *nb_test_other(void);int nb_test_ok(void);
void gtk_widget_set_has_tooltip(void*,int);void gtk_widget_trigger_tooltip_query(void*);void gtk_tooltip_set_text(void*,const char*);
static void arm(void){int f=open(getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM"),O_CREAT|O_EXCL|O_WRONLY,0600);if(f<0)exit(11);close(f);}
int main(void){void*w=nb_test_widget();gtk_widget_set_has_tooltip(w,0);gtk_widget_trigger_tooltip_query(w);arm();gtk_widget_set_has_tooltip(nb_test_other(),0);gtk_widget_set_has_tooltip(w,1);gtk_widget_trigger_tooltip_query(w);gtk_tooltip_set_text(w,"PRIVATE_SENTINEL_URL");unlink(getenv("NEXUSBRO_TEST_TOOLTIP_TRACE_ARM"));fputs("UNIT_ARM_REMOVED\n",stderr);for(int i=0;i<10;i++)gtk_widget_set_has_tooltip(w,0);fputs("UNIT_ARM_RESTORED\n",stderr);arm();for(int i=0;i<200;i++)gtk_widget_set_has_tooltip(w,i&1);return nb_test_ok()?0:12;}
'''
with tempfile.TemporaryDirectory(prefix='nb-gtk-abi-') as value:
    root=Path(value);(root/'fake.c').write_text(LIB);(root/'main.c').write_text(MAIN)
    def build(args):subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',*args],check=True,capture_output=True,timeout=20)
    build(['-shared','-fPIC',str(root/'fake.c'),'-o',str(root/'libfakegtk.so')])
    build([str(root/'main.c'),'-L'+str(root),'-lfakegtk','-Wl,-rpath,'+str(root),'-o',str(root/'stub')])
    build(['-shared','-fPIC',str(ROOT/'tests/gui/gtk_tooltip_trace.c'),'-ldl','-pthread','-o',str(root/'trace.so')])
    env={**os.environ,'LD_PRELOAD':str(root/'trace.so'),'GITHUB_ACTIONS':'true','NEXUSBRO_TEST_DATA':str(root),'NEXUSBRO_TEST_TOOLTIP_TRACE_ARM':str(root/'armed')}
    result=subprocess.run([str(root/'stub')],env=env,capture_output=True,text=True,timeout=5)
    assert result.returncode==0, 'Native calls did not forward exactly'
    assert 'PRIVATE_SENTINEL_URL' not in result.stdout+result.stderr
    inactive=result.stderr.split('UNIT_ARM_REMOVED\n')[1].split('UNIT_ARM_RESTORED\n')[0]
    assert 'NB_TOOLTIP_TRACE' not in inactive, 'Removed arm marker did not disable logging'
    records=[line for line in result.stderr.splitlines() if line.startswith('NB_TOOLTIP_TRACE ')]
    assert len(records)==64, 'Bounded trace/rearming contract failed'
    assert any('event=trigger observed=1 requested=1 has=1 enabled=1 timeout_ms=500 browse_ms=60' in x for x in records)
    assert any('event=text observed=0' in x and 'text_present=1' in x for x in records)
print(json.dumps({'stub_abi_test':True,'exact_forwarding':True,'rearming_and_log_cap':True,'private_text_not_exported':True,'gtk_or_browser_executed':False,'native_acceptance':False}))
