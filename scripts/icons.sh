#!/bin/bash
# Draws the tray/app icons (assets/dfc-active.ico, dfc-paused.ico) with ImageMagick: scripts/icons.sh <tmpdir> assets
# usage: icons.sh outdir assetsdir
set -e
out=$1; assets=$2; mkdir -p "$assets/linux"
f() { awk "BEGIN{printf \"%g\", $1}"; }
for n in active paused; do
  if [ $n = active ]; then c='#2f7fd8'; else c='#8a8f98'; fi
  files=()
  for s in 16 20 24 32 48 64; do
    k=$(f "$s/32"); w=$(f "($s<=24)?2:2.5*$k")
    args=(-size ${s}x${s} xc:none
      -fill none -stroke "$c" -strokewidth "$w"
      -draw "roundrectangle $(f 3*$k),$(f 5*$k) $(f 29*$k),$(f 22*$k) $(f 2*$k),$(f 2*$k)"
      -fill "$c" -stroke none
      -draw "rectangle $(f 14*$k),$(f 22*$k) $(f 18*$k),$(f 26*$k)"
      -draw "rectangle $(f 9*$k),$(f 26*$k) $(f 23*$k),$(f 28.5*$k)")
    if [ $n = active ]; then
      args+=(-draw "polygon $(f 12.5*$k),$(f 9*$k) $(f 12.5*$k),$(f 18*$k) $(f 20.5*$k),$(f 13.5*$k)")
    else
      args+=(-draw "rectangle $(f 11.5*$k),$(f 9.5*$k) $(f 14.5*$k),$(f 17.5*$k)"
             -draw "rectangle $(f 17.5*$k),$(f 9.5*$k) $(f 20.5*$k),$(f 17.5*$k)")
    fi
    magick "${args[@]}" "$out/$n-$s.png"
    [ $s -le 48 ] && files+=("$out/$n-$s.png")
  done
  magick "${files[@]}" "$assets/dfc-$n.ico"
done
magick "$out"/active-32.png "$out"/paused-32.png "$out"/active-64.png "$out"/paused-64.png +append "$out/preview.png"
# Linux tray (StatusNotifierItem wants raw pixels): RGBA, converted to ARGB at runtime.
for n in active paused; do for s in 24 48; do
  magick "$out/$n-$s.png" -depth 8 "rgba:$assets/linux/$n-$s.rgba"
done; done
