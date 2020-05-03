#!/usr/bin/env sh
while :
do
  when=`date -Iseconds`
  cargo run --release | ztee "rollchan-$when.log"
done

