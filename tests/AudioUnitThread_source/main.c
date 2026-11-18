/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include <mach/kern_return.h>
#include <mach/thread_info.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// The common-3.0 SDK lacks these Audio Component declarations.
typedef void *AudioComponent;
typedef void *AudioUnit;
typedef struct {
  uint32_t type, subtype, manufacturer, flags, flags_mask;
} AudioComponentDescription;
typedef struct {
  double sample_rate;
  uint32_t format_id, flags, bytes_per_packet, frames_per_packet;
  uint32_t bytes_per_frame, channels, bits_per_channel, reserved;
} AudioStreamBasicDescription;
typedef struct {
  uint32_t channels, size;
  void *data;
} AudioBuffer;
typedef struct {
  uint32_t count;
  AudioBuffer buffers[1];
} AudioBufferList;
typedef int32_t (*RenderCallback)(void *, uint32_t *, const void *, uint32_t,
                                  uint32_t, AudioBufferList *);
typedef struct {
  RenderCallback callback;
  void *context;
} RenderCallbackStruct;
typedef struct {
  uint32_t seconds, nanoseconds;
} MachTimespec;

extern AudioComponent AudioComponentFindNext(AudioComponent,
                                             const AudioComponentDescription *);
extern int32_t AudioComponentInstanceNew(AudioComponent, AudioUnit *);
extern int32_t AudioComponentInstanceDispose(AudioUnit);
extern int32_t AudioUnitSetProperty(AudioUnit, uint32_t, uint32_t, uint32_t,
                                    const void *, uint32_t);
extern int32_t AudioUnitInitialize(AudioUnit);
extern int32_t AudioUnitUninitialize(AudioUnit);
extern int32_t AudioOutputUnitStart(AudioUnit);
extern int32_t AudioOutputUnitStop(AudioUnit);
extern mach_port_t mach_task_self_;
extern kern_return_t semaphore_create(mach_port_t, mach_port_t *, int, int);
extern kern_return_t semaphore_destroy(mach_port_t, mach_port_t);
extern kern_return_t semaphore_signal(mach_port_t);
extern kern_return_t semaphore_timedwait(mach_port_t, MachTimespec);

static mach_port_t notification;
static pthread_t main_thread, render_thread;
static volatile unsigned callbacks;
static volatile int wrong_thread;

// Render callbacks live on an asynchronous audio thread, including while the
// main thread is blocked. Story Mode's FMOD bank load depends on this.
// https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/AudioUnitHostingGuide_iOS/AudioUnitHostingFundamentals/AudioUnitHostingFundamentals.html
static int32_t render(void *context, uint32_t *flags, const void *timestamp,
                      uint32_t bus, uint32_t frames, AudioBufferList *buffers) {
  (void)context;
  (void)flags;
  (void)timestamp;
  (void)bus;
  (void)frames;
  pthread_t current = pthread_self();
  if (current == main_thread || (render_thread && current != render_thread))
    wrong_thread = 1;
  render_thread = current;
  for (unsigned i = 0; i < buffers->count; i++)
    memset(buffers->buffers[i].data, 0, buffers->buffers[i].size);
  if (++callbacks == 1)
    semaphore_signal(notification);
  return 0;
}

static int probe(void) {
  AudioComponentDescription description = {0x61756f75, 0x72696f63, 0x6170706c,
                                           0, 0};
  AudioComponent component = AudioComponentFindNext(NULL, &description);
  AudioUnit unit = NULL;
  if (!component || AudioComponentInstanceNew(component, &unit))
    return 1;
  AudioStreamBasicDescription format = {48000, 0x6c70636d, 12, 4, 1,
                                        4,     2,          16, 0};
  RenderCallbackStruct callback = {render, NULL};
  if (AudioUnitSetProperty(unit, 8, 1, 0, &format, sizeof(format)) ||
      AudioUnitSetProperty(unit, 23, 1, 0, &callback, sizeof(callback)) ||
      AudioUnitInitialize(unit))
    return 2;
  main_thread = pthread_self();
  if (semaphore_create(mach_task_self_, &notification, 0, 0) != KERN_SUCCESS)
    return 3;
  for (unsigned attempt = 0; attempt < 2; attempt++) {
    callbacks = 0;
    // Starting an already-running unit must not spawn another render worker.
    if (AudioOutputUnitStart(unit) || AudioOutputUnitStart(unit))
      return 4;
    MachTimespec timeout = {1, 0};
    // No NSRunLoop is run here: the callback must signal this blocked thread.
    if (semaphore_timedwait(notification, timeout) != KERN_SUCCESS ||
        callbacks == 0 || wrong_thread)
      return 5;
    if (AudioOutputUnitStop(unit))
      return 6;
    unsigned stopped_count = callbacks;
    usleep(30000);
    if (callbacks != stopped_count)
      return 7;
  }
  if (AudioUnitUninitialize(unit) || AudioComponentInstanceDispose(unit) ||
      semaphore_destroy(mach_task_self_, notification) != KERN_SUCCESS)
    return 8;
  // Let the disposed unit's worker exit, exposing use-after-disposal bugs.
  usleep(30000);
  return 0;
}

int main(void) {
  int result = probe();
  printf("Audio Unit background callback test: %s (%d)\n",
         result == 0 ? "PASS" : "FAIL", result);
  return result;
}
