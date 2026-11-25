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
EXTRA=r'''
typedef int(*Hook)(void*,unsigned,const void*,void*);
typedef void(*Destroy)(void*);
static Hook hooks[3];static void*hook_data[3];static int c,mapped,visible;
struct Value {unsigned long g_type;void*object;};static const void*expected_value;
unsigned long gtk_widget_get_type(void){return 100;}
unsigned long gtk_window_get_type(void){return 200;}
int g_type_check_instance_is_a(void*w,unsigned long t){return w==&c&&t==200;}
int gtk_window_get_type_hint(void*w){if(w!=&c)abort();return 10;}
int gtk_widget_get_mapped(void*w){return w==&a?1:(w==&c?mapped:0);}
int gtk_widget_get_visible(void*w){return w==&a?1:(w==&c?visible:0);}
void*g_value_get_object(const void*v){if(v!=expected_value)abort();return ((const struct Value*)v)->object;}
unsigned g_signal_lookup(const char*n,unsigned long t){if(t!=100)abort();return !strcmp(n,"realize")?1:!strcmp(n,"map")?2:!strcmp(n,"unmap")?3:0;}
unsigned long g_signal_add_emission_hook(unsigned id,unsigned detail,Hook hook,void*data,Destroy destroy){if(!id||id>3||detail||!hook||destroy)abort();if(getenv("NB_UNIT_FAIL_HOOK")&&id==2)return 0;hooks[id-1]=hook;hook_data[id-1]=data;return id;}
void nb_test_emit(unsigned event){unsigned id=event<=3?event:2;struct Value v={0xdeadbeef,event==5?(void*)&b:(void*)&c};expected_value=&v;mapped=id==2;visible=id!=3;if(hooks[id-1]&&hooks[id-1](0,event==4?2:1,&v,hook_data[id-1])!=1)abort();expected_value=0;}
'''
MAIN_HOOKS=MAIN.replace('void *nb_test_widget(void);','void nb_test_emit(unsigned);void *nb_test_widget(void);').replace('gtk_tooltip_set_text(w,"PRIVATE_SENTINEL_URL");','gtk_tooltip_set_text(w,"PRIVATE_SENTINEL_URL");nb_test_emit(1);nb_test_emit(2);nb_test_emit(3);nb_test_emit(4);nb_test_emit(5);')
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
    (root/'fake.c').write_text(LIB+EXTRA);(root/'main.c').write_text(MAIN_HOOKS)
    build(['-shared','-fPIC',str(root/'fake.c'),'-o',str(root/'libfakegtk.so')])
    build([str(root/'main.c'),'-L'+str(root),'-lfakegtk','-Wl,-rpath,'+str(root),'-o',str(root/'stub')])
    for failed in (False,True):
        (root/'armed').unlink()
        if failed:env['NB_UNIT_FAIL_HOOK']='1'
        result=subprocess.run([str(root/'stub')],env=env,capture_output=True,text=True,timeout=5)
        assert result.returncode==0, 'GValue callback ABI or forwarding failed'
        records=[x for x in result.stderr.splitlines() if x.startswith('NB_TOOLTIP_TRACE ')]
        assert len(records)==64 and 'PRIVATE_SENTINEL_URL' not in result.stderr
        assert all(('hooks=0' if failed else 'hooks=1') in x for x in records)
        if not failed:
            assert len([x for x in records if 'event=map ' in x])==1
            assert any('event=map ' in x and 'mapped=1 visible=1 hooks=1' in x for x in records)
            assert any('event=unmap ' in x and 'mapped=0 visible=0 hooks=1' in x for x in records)
print(json.dumps({'stub_abi_test':True,'exact_forwarding':True,'rearming_and_log_cap':True,'private_text_not_exported':True,'lifecycle_callback_abi':True,'partial_hook_registration_reported':True,'gtk_or_browser_executed':False,'native_acceptance':False}))
