This is a python daemon that exposes fan and light controls for the Casa Vieja fans to Home Assistant. The remote is called `Ceiling fan remote controller`, model is `TR301A`. The remote has a big SET button, but no DIP switches that can be configured for different fans.

Decoding the remote control signal and remote ids
--

I had to use a RTL-SDR dongle to find out the serial number for the remotes. Please follow [this wonderful tutorial](https://www.youtube.com/watch?v=_GCpqory3kc) to understand how to capture and decode the ceiling fan signal. 

Sending the remote control signal
--

You can use a vanilla raspberry pi of any kind to transmit control signals (I used a Pi 0), using [rpitx](https://github.com/F5OEO/rpitx), at least in a hacky way, [without additional or customized hardware](https://www.youtube.com/watch?v=3lGU7PjJM7k). 

Note that this only serves as a remote control, as we doesn't know what the fans are actually doing, we just send commands blindly and hope it works just like the remotes.

How to use this repository
--

Install prereqs:

```
sudo apt install python3-paho-mqtt python3-yaml
```

Enter your remote ids into the config file, you should be ready to go. The easiest is to run the `onlyfansd` directly with python3:

```
python3 onlyfansd.py
```

Just add the MQTT integration to HomeAssistant, you'll see the fans you configured appearing as a fan entity and a light entity. 

If you can't control To debug, you should compare the signal that this daemon sends out and the actual signal that the remote sends out to understand what went wrong. Or you could spend some more minutes to figure out how the SET button works, and then add a program button to the integration. I can't be bothered to figure out how the SET button works, to send arbitrary ID we choose to the fans, but [I assume that's possible](https://www.amazon.com/review/R2VWOTH0LUT4XJ/).

