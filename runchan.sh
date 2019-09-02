#!/usr/bin/env sh
while :
do
  when=`date -Iseconds`
  cargo run --release | tee "rollchan-$when.log"
done

