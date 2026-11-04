;===----------------------------------------------------------------------===;
; SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
;===----------------------------------------------------------------------===;
; Private compiler implementation. Strip these source comments from output.
@__briev_region_current = internal thread_local global ptr null
@__briev_region_active = internal thread_local global ptr null

define internal void @__briev_region_enter(ptr %frame) {
entry:
  %parent = load ptr, ptr @__briev_region_current
  store ptr %parent, ptr %frame
  %head = getelementptr { ptr, ptr }, ptr %frame, i32 0, i32 1
  store ptr null, ptr %head
  %active = load ptr, ptr @__briev_region_active
  %active_slot = getelementptr { ptr, ptr, ptr }, ptr %frame, i32 0, i32 2
  store ptr %active, ptr %active_slot
  store ptr %frame, ptr @__briev_region_active
  store ptr %frame, ptr @__briev_region_current
  ret void
}

define internal ptr @__briev_region_suspend() {
entry:
  %parent = load ptr, ptr @__briev_region_current
  store ptr null, ptr @__briev_region_current
  ret ptr %parent
}

define internal void @__briev_region_restore(ptr %parent) {
entry:
  store ptr %parent, ptr @__briev_region_current
  ret void
}

define internal ptr @__briev_region_malloc(i64 %size) {
entry:
  %data = call ptr @malloc(i64 %size)
  %frame = load ptr, ptr @__briev_region_current
  %inactive = icmp eq ptr %frame, null
  %failed = icmp eq ptr %data, null
  %skip = or i1 %inactive, %failed
  br i1 %skip, label %done, label %track
track:
  %record = call ptr @malloc(i64 16)
  %record_failed = icmp eq ptr %record, null
  br i1 %record_failed, label %fail, label %link
fail:
  call void @free(ptr %data)
  ret ptr null
link:
  %head = getelementptr { ptr, ptr }, ptr %frame, i32 0, i32 1
  %previous = load ptr, ptr %head
  store ptr %previous, ptr %record
  %value = getelementptr { ptr, ptr }, ptr %record, i32 0, i32 1
  store ptr %data, ptr %value
  store ptr %record, ptr %head
  br label %done
done:
  ret ptr %data
}

define internal void @__briev_region_free(ptr %data) {
entry:
  %initial = load ptr, ptr @__briev_region_active
  br label %frames
frames:
  %frame = phi ptr [ %initial, %entry ], [ %parent, %nextframe ]
  %no_frame = icmp eq ptr %frame, null
  br i1 %no_frame, label %release, label %head
head:
  %head_slot = getelementptr { ptr, ptr }, ptr %frame, i32 0, i32 1
  br label %records
records:
  %slot = phi ptr [ %head_slot, %head ], [ %record, %advance ]
  %record = load ptr, ptr %slot
  %no_record = icmp eq ptr %record, null
  br i1 %no_record, label %nextframe, label %compare
compare:
  %value_slot = getelementptr { ptr, ptr }, ptr %record, i32 0, i32 1
  %value = load ptr, ptr %value_slot
  %found = icmp eq ptr %value, %data
  br i1 %found, label %unlink, label %advance
advance:
  br label %records
unlink:
  %next = load ptr, ptr %record
  store ptr %next, ptr %slot
  call void @free(ptr %record)
  br label %release
nextframe:
  %parent_slot = getelementptr { ptr, ptr, ptr }, ptr %frame, i32 0, i32 2
  %parent = load ptr, ptr %parent_slot
  br label %frames
release:
  call void @free(ptr %data)
  ret void
}

define internal ptr @__briev_region_realloc(ptr %data, i64 %size) {
entry:
  %empty = icmp eq ptr %data, null
  br i1 %empty, label %allocate, label %resize
allocate:
  %fresh = call ptr @__briev_region_malloc(i64 %size)
  ret ptr %fresh
resize:
  %resized = call ptr @realloc(ptr %data, i64 %size)
  %failed = icmp eq ptr %resized, null
  %zero = icmp eq i64 %size, 0
  %retain = xor i1 %zero, true
  %keep_old = and i1 %failed, %retain
  br i1 %keep_old, label %done, label %start
start:
  %initial = load ptr, ptr @__briev_region_active
  br label %frames
frames:
  %frame = phi ptr [ %initial, %start ], [ %parent, %nextframe ]
  %no_frame = icmp eq ptr %frame, null
  br i1 %no_frame, label %done, label %head
head:
  %head_slot = getelementptr { ptr, ptr }, ptr %frame, i32 0, i32 1
  br label %records
records:
  %slot = phi ptr [ %head_slot, %head ], [ %record, %advance ]
  %record = load ptr, ptr %slot
  %no_record = icmp eq ptr %record, null
  br i1 %no_record, label %nextframe, label %compare
compare:
  %value_slot = getelementptr { ptr, ptr }, ptr %record, i32 0, i32 1
  %value = load ptr, ptr %value_slot
  %found = icmp eq ptr %value, %data
  br i1 %found, label %update, label %advance
advance:
  br label %records
update:
  store ptr %resized, ptr %value_slot
  br label %done
nextframe:
  %parent_slot = getelementptr { ptr, ptr, ptr }, ptr %frame, i32 0, i32 2
  %parent = load ptr, ptr %parent_slot
  br label %frames
done:
  ret ptr %resized
}

define internal void @__briev_region_leave(ptr %frame) {
entry:
  %head = getelementptr { ptr, ptr }, ptr %frame, i32 0, i32 1
  %initial = load ptr, ptr %head
  br label %records
records:
  %record = phi ptr [ %initial, %entry ], [ %next, %release ]
  %empty = icmp eq ptr %record, null
  br i1 %empty, label %done, label %release
release:
  %next = load ptr, ptr %record
  %value_slot = getelementptr { ptr, ptr }, ptr %record, i32 0, i32 1
  %value = load ptr, ptr %value_slot
  call void @free(ptr %value)
  call void @free(ptr %record)
  br label %records
done:
  %parent = load ptr, ptr %frame
  store ptr %parent, ptr @__briev_region_current
  %active_slot = getelementptr { ptr, ptr, ptr }, ptr %frame, i32 0, i32 2
  %active = load ptr, ptr %active_slot
  store ptr %active, ptr @__briev_region_active
  ret void
}

define internal void @__briev_region_finish() {
entry:
  br label %frames
frames:
  %frame = load ptr, ptr @__briev_region_active
  %empty = icmp eq ptr %frame, null
  br i1 %empty, label %done, label %release
release:
  call void @__briev_region_leave(ptr %frame)
  br label %frames
done:
  ret void
}
