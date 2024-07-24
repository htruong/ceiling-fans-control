import paho.mqtt.client as mqtt
import json
import yaml
import logging
import time
from control_fan import control_fan



# Set up logging
logging.basicConfig(level=logging.info, format='%(asctime)s - %(levelname)s - %(message)s')

# Load configuration
with open('config.yaml', 'r') as config_file:
    config = yaml.safe_load(config_file)

MQTT_BROKER = config['mqtt']['broker']
MQTT_PORT = config['mqtt']['port']
MQTT_USERNAME = config.get('mqtt', {}).get('username')
MQTT_PASSWORD = config.get('mqtt', {}).get('password')
FANS = config['fans']

# MQTT topics
DISCOVERY_PREFIX = 'homeassistant'
COMMAND_TOPIC = 'set'
STATE_TOPIC = 'state'
AVAILABILITY_TOPIC = 'availability'

def on_connect(client, userdata, flags, rc):
    logging.info(f"Connected with result code {rc}")
    client.publish(f"{DISCOVERY_PREFIX}/status", "online", retain=True)

    # Publish initial states for all fans and lights
    publish_initial_states(client)

    # Publish discovery messages and subscribe to topics for each room
    for room in FANS:
	# Clear retained messages
        client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/on/{COMMAND_TOPIC}", "", retain=True)
        client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/speed/percentage/{COMMAND_TOPIC}", "", retain=True)
        client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/direction/{COMMAND_TOPIC}", "", retain=True)
        client.publish(f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{COMMAND_TOPIC}", "", retain=True)
        logging.info(f"Cleared all previous states and commands for {room}")

        publish_discovery_fan(client, room)
        publish_discovery_light(client, room)
        logging.info(f"Published all devices")

        # Subscribe to fan topics
        fan_command_topic = f"{DISCOVERY_PREFIX}/fan/{room}_fan/on/{COMMAND_TOPIC}"
        fan_percentage_topic = f"{DISCOVERY_PREFIX}/fan/{room}_fan/speed/percentage/{COMMAND_TOPIC}"
        fan_direction_topic = f"{DISCOVERY_PREFIX}/fan/{room}_fan/direction/{COMMAND_TOPIC}"
        client.subscribe(fan_command_topic)
        client.subscribe(fan_percentage_topic)
        client.subscribe(fan_direction_topic)
        logging.info(f"Subscribed to fan topics: {fan_command_topic}, {fan_percentage_topic}, {fan_direction_topic}")

        # Subscribe to light topic
        light_command_topic = f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{COMMAND_TOPIC}"
        client.subscribe(light_command_topic)
        logging.info(f"Subscribed to light topic: {light_command_topic}")

    logging.info("All discovery messages published and topics subscribed")

def on_message(client, userdata, msg):
    logging.info(f"Received message on topic: {msg.topic}")
    logging.info(f"Message payload: {msg.payload}")

    try:
        topic_parts = msg.topic.split('/')
        room = topic_parts[2].replace('_fan', '').replace('_light', '')
        device_type = 'light' if 'light' in topic_parts[2] else 'fan'
        payload = msg.payload.decode()
        state_payload = {}
        logging.info(f"Received command for {room} {device_type}: {payload}")

        if device_type == 'fan':
            if '/speed/percentage/' in msg.topic:
                percentage = int(payload)
                snapped_speed = round(((100 - percentage) / 100) * 6) # 0->6, 0 is fastest
                if snapped_speed == 6:
                    control_fan(room, 'stop')
                    state_payload = {
                        'state': 'OFF',
                        'percentage': 0
                    }
                else:
                    speed = 'speed' + str(snapped_speed + 1)
                    snapped_pct = round(100 - (snapped_speed * 100/6)) 
                    control_fan(room, speed)
                    state_payload = {
                        'state': 'ON',
                        'percentage': snapped_pct
                    }
            elif '/direction/' in msg.topic:
                control_fan(room, 'reverse')
                state_payload = {
                    'direction': payload
                }
            elif '/on/' in msg.topic:
                control_fan(room, 'stop' if payload == 'OFF' else 'speed4')
                state_payload = {
                    'state': payload,
                    'percentage': 33
                }
            else:
                logging.info(f"Payload WTF?")

            client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}", json.dumps(state_payload), retain=True)

        elif device_type == 'light':
            try:
                payload_dict = json.loads(payload)
            except json.JSONDecodeError:
                payload_dict = {"state": payload}

            if 'state' in payload_dict:
                if payload_dict['state'] in ['ON', 'OFF']:
                    control_fan(room, 'light')

            state_payload = {
                'state': payload_dict.get('state', 'ON')
            }
            client.publish(f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{STATE_TOPIC}", json.dumps(state_payload), retain=True)

    except Exception as e:
        logging.error(f"Error processing message: {e}")


def publish_initial_states(client):
    logging.info("Publishing initial states for all fans and lights")
    logging.info(f"FANS: {FANS}")
    for room in FANS:
        # Publish initial state for fan
        fan_initial_state = {
            'state': 'OFF',
            'percentage': 0,
            'direction': 'forward'
        }
        fan_state_topic = f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}"
        logging.info(f"Publishing initial state for fan {room}: {fan_initial_state}")
        client.publish(fan_state_topic, json.dumps(fan_initial_state), retain=True)
        logging.info(f"Published initial state for fan {room}: {fan_initial_state}")

        # Publish initial state for light
        light_initial_state = {
            'state': 'OFF'
        }
        light_state_topic = f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{STATE_TOPIC}"
        logging.info(f"Publishing initial state for light {room}: {light_initial_state}")
        client.publish(light_state_topic, json.dumps(light_initial_state), retain=True)
        logging.info(f"Published initial state for light {room}: {light_initial_state}")
    logging.info("Publishing initial states for all fans and lights - DONE")

def publish_discovery_fan(client, room):
    logging.info(f"Publish fan: {room}...")
    payload = {
        "name": f"{room.replace('_', ' ').title()} Fan",
        "unique_id": f"fan_{room}",
        "command_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/on/{COMMAND_TOPIC}",
        "state_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}",
        "availability_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/{AVAILABILITY_TOPIC}",
        "payload_available": "online",
        "payload_not_available": "offline",
        "state_value_template": "{{ value_json.state }}",
        "percentage_command_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/speed/percentage/{COMMAND_TOPIC}",
        "percentage_command_template": "{{ value }}",
        "percentage_state_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}",
        "percentage_value_template": "{{ value_json.percentage }}",
        "direction_command_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/direction/{COMMAND_TOPIC}",
        "direction_state_topic": f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}",
        "direction_value_template": "{{ value_json.direction }}",
        "optimistic": False,
        "qos": 0,
        "retain": True
    }
    discovery_topic = f"{DISCOVERY_PREFIX}/fan/{room}_fan/config"
    logging.info(f"Publishing fan discovery message to topic: {discovery_topic}")
    logging.info(f"Fan discovery payload: {json.dumps(payload)}")
    client.publish(discovery_topic, json.dumps(payload), retain=True)
    client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/{AVAILABILITY_TOPIC}", "online", retain=True)

    # Publish initial state
    initial_state = {
        'state': 'OFF',
        'percentage': 0,
        'direction': 'forward'
    }
    client.publish(f"{DISCOVERY_PREFIX}/fan/{room}_fan/{STATE_TOPIC}", json.dumps(initial_state), retain=True)

def publish_discovery_light(client, room):
    logging.info(f"Publish fan light: {room}...")
    payload = {
        "name": f"{room.replace('_', ' ').title()} Fan Light",
        "unique_id": f"light_{room}_fan",
        "command_topic": f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{COMMAND_TOPIC}",
        "state_topic": f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{STATE_TOPIC}",
        "availability_topic": f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{AVAILABILITY_TOPIC}",
        "payload_available": "online",
        "payload_not_available": "offline",
        "state_value_template": "{{ value_json.state }}",
        "optimistic": False,
        "qos": 0,
        "retain": True
    }
    discovery_topic = f"{DISCOVERY_PREFIX}/light/{room}_fan_light/config"
    logging.info(f"Publishing light discovery message to topic: {discovery_topic}")
    logging.info(f"Light discovery payload: {json.dumps(payload)}")
    client.publish(discovery_topic, json.dumps(payload), retain=True)
    client.publish(f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{AVAILABILITY_TOPIC}", "online", retain=True)

    # Publish initial state
    initial_state = {
        'state': 'OFF'
    }
    client.publish(f"{DISCOVERY_PREFIX}/light/{room}_fan_light/{STATE_TOPIC}", json.dumps(initial_state), retain=True)


def on_subscribe(client, userdata, mid, granted_qos):
    logging.info(f"Subscribed successfully. Message ID: {mid}, Granted QoS: {granted_qos}")

def main():
    client = mqtt.Client(clean_session=True)
    client.on_connect = on_connect
    client.on_message = on_message
    client.on_subscribe = on_subscribe

    if MQTT_USERNAME and MQTT_PASSWORD:
        client.username_pw_set(MQTT_USERNAME, MQTT_PASSWORD)

    while True:
        try:
            logging.info(f"Attempting to connect to MQTT broker at {MQTT_BROKER}:{MQTT_PORT}")
            client.connect(MQTT_BROKER, MQTT_PORT, 60)
            client.loop_forever()
        except Exception as e:
            logging.error(f"Connection failed: {e}")
            time.sleep(10)  # Wait before trying to reconnect

if __name__ == "__main__":
    main()

