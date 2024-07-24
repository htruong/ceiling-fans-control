This is a python daemon that exposes fan controls for the Casa Vieja fans to Home Assistant. The remote is called `Ceiling fan remote controller`, model is `TR301A`. The remote has a big SET button, but no DIP switches that can be configured for different fans.

Note that this only serves as a remote control, as we doesn't know what the fans are actually doing, we just send commands blindly and hope it works just like the remotes.

You can use a vanilla raspberry pi of any kind to transmit control signals (I used a Pi 0), using [https://github.com/F5OEO/rpitx](rpitx), without additional or customized hardware.

I had to use a RTL-SDR dongle to find out the serial number for the remotes. Please follow [this wonderful tutorial](https://www.youtube.com/watch?v=_GCpqory3kc) to understand how to capture and decode the ceiling fan signal. I can't be bothered to figure out how the SET button works, to send the ID we chose to the fans, but [I assume that's possible](https://www.amazon.com/review/R2VWOTH0LUT4XJ/).

Enter your remote ids into the config file, you should be ready to go. To debug, you should compare the signal that this daemon sends out and the actual signal that the remote sends out to understand what went wrong.
