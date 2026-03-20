import json
import yaml
import logging
import time
import requests
import websocket
from threading import Timer

from control_fan import control_fan


logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

with open('config.yaml', 'r') as config_file:
    config = yaml.safe_load(config_file)

HA_URL = config['homeassistant']['url'].rstrip('/')
HA_TOKEN = config['homeassistant']['token']
FANS = config['fans']

fan_states = {}
light_states = {}
fan_percentage_requests = {}
homekit_bridge = None


def ha_set_state(entity_id, state, attributes=None):
    headers = {
        'Authorization': f'Bearer {HA_TOKEN}',
        'Content-Type': 'application/json',
    }
    body = {'state': state, 'attributes': attributes or {}}
    try:
        r = requests.post(f'{HA_URL}/api/states/{entity_id}', headers=headers, json=body, timeout=5)
        r.raise_for_status()
    except Exception as e:
        logging.error(f"Failed to set state for {entity_id}: {e}")


def set_fan_state(room, state):
    fan_states[room] = state
    if homekit_bridge:
        homekit_bridge.update_fan(room, state)
    ha_set_state(
        f'fan.{room}_fan',
        state['state'].lower(),
        {
            'percentage': state['percentage'],
            'direction': state.get('direction', 'forward'),
            'supported_features': 5,  # SET_SPEED + DIRECTION
            'friendly_name': room.replace('_', ' ').title() + ' Fan',
        }
    )


def set_light_state(room, state):
    light_states[room] = state
    if homekit_bridge:
        homekit_bridge.update_light(room, state)
    ha_set_state(
        f'light.{room}_fan_light',
        state['state'].lower(),
        {'friendly_name': room.replace('_', ' ').title() + ' Fan Light'}
    )


def init_entities():
    logging.info("Initialising entities in Home Assistant...")
    for room in FANS:
        set_fan_state(room, {'state': 'OFF', 'percentage': 0, 'direction': 'forward'})
        set_light_state(room, {'state': 'OFF'})
    logging.info("Entities initialised")


def delayed_fan_speed_change(room, percentage):
    fan_percentage_requests.pop(room, None)
    change_fan_speed_pct(room, percentage)


def schedule_fan_speed_change(room, percentage):
    if room in fan_percentage_requests:
        fan_percentage_requests[room].cancel()

    fan_percentage_requests[room] = Timer(1.5, delayed_fan_speed_change, args=[room, percentage])
    fan_percentage_requests[room].start()

    # Immediate snapped feedback
    snapped_speed = round(((100 - percentage) / 100) * 6)
    direction = fan_states.get(room, {}).get('direction', 'forward')
    if snapped_speed == 6:
        set_fan_state(room, {'state': 'OFF', 'percentage': 0, 'direction': direction})
    else:
        snapped_pct = round(100 - (snapped_speed * 100 / 6))
        set_fan_state(room, {'state': 'ON', 'percentage': snapped_pct, 'direction': direction})


def change_fan_speed_pct(room, percentage):
    snapped_speed = round(((100 - percentage) / 100) * 6)
    direction = fan_states.get(room, {}).get('direction', 'forward')
    if snapped_speed == 6:
        control_fan(room, 'stop')
        set_fan_state(room, {'state': 'OFF', 'percentage': 0, 'direction': direction})
    else:
        speed = 'speed' + str(snapped_speed + 1)
        snapped_pct = round(100 - (snapped_speed * 100 / 6))
        logging.info(f"Fan {room} snapped to {speed} ({snapped_pct}%)")
        control_fan(room, speed)
        set_fan_state(room, {'state': 'ON', 'percentage': snapped_pct, 'direction': direction})


def handle_service_call(domain, service, entity_ids, service_data):
    if isinstance(entity_ids, str):
        entity_ids = [entity_ids]

    for entity_id in entity_ids:
        if domain == 'fan' and entity_id.startswith('fan.') and entity_id.endswith('_fan'):
            room = entity_id[len('fan.'):-len('_fan')]
            if room not in FANS:
                continue
            logging.info(f"Fan command: {room} {service} {service_data}")
            if service == 'turn_on':
                schedule_fan_speed_change(room, service_data.get('percentage', 35))
            elif service == 'turn_off':
                schedule_fan_speed_change(room, 0)
            elif service == 'set_percentage':
                schedule_fan_speed_change(room, service_data.get('percentage', 0))
            elif service == 'set_direction':
                control_fan(room, 'reverse')
                current = fan_states.get(room, {'state': 'OFF', 'percentage': 0, 'direction': 'forward'})
                current['direction'] = service_data.get('direction', 'forward')
                set_fan_state(room, current)

        elif domain == 'light' and entity_id.startswith('light.') and entity_id.endswith('_fan_light'):
            room = entity_id[len('light.'):-len('_fan_light')]
            if room not in FANS:
                continue
            logging.info(f"Light command: {room} {service}")
            if service in ('turn_on', 'turn_off', 'toggle'):
                control_fan(room, 'light')
                current_on = light_states.get(room, {}).get('state', 'OFF') == 'ON'
                if service == 'turn_on':
                    new_state = 'ON'
                elif service == 'turn_off':
                    new_state = 'OFF'
                else:
                    new_state = 'OFF' if current_on else 'ON'
                set_light_state(room, {'state': new_state})


ws_msg_id = 1

def on_ws_message(ws, message):
    global ws_msg_id
    data = json.loads(message)
    msg_type = data.get('type')

    if msg_type == 'auth_required':
        ws.send(json.dumps({'type': 'auth', 'access_token': HA_TOKEN}))

    elif msg_type == 'auth_ok':
        logging.info("Authenticated with Home Assistant WebSocket")
        ws_msg_id += 1
        ws.send(json.dumps({'id': ws_msg_id, 'type': 'subscribe_events', 'event_type': 'call_service'}))

    elif msg_type == 'auth_invalid':
        logging.error("Home Assistant authentication failed — check your token")
        ws.close()

    elif msg_type == 'event':
        event_data = data.get('event', {}).get('data', {})
        domain = event_data.get('domain', '')
        service = event_data.get('service', '')
        service_data = event_data.get('service_data', {})

        # HA 2022+ puts targets in 'target', older versions put entity_id in service_data
        target = event_data.get('target', {})
        entity_ids = target.get('entity_id') or service_data.get('entity_id', [])

        if domain in ('fan', 'light') and entity_ids:
            handle_service_call(domain, service, entity_ids, service_data)

    elif msg_type == 'result' and not data.get('success'):
        logging.warning(f"WebSocket command failed: {data}")


def on_ws_error(ws, error):
    logging.error(f"WebSocket error: {error}")


def on_ws_close(ws, close_status_code, close_msg):
    logging.info(f"WebSocket closed (code={close_status_code})")


def main():
    global homekit_bridge
    hk_config = config.get('homekit')
    if hk_config and hk_config.get('enabled'):
        from homekit_bridge import HomeKitBridge
        homekit_bridge = HomeKitBridge(
            fans=FANS,
            port=hk_config.get('port', 51826),
            persist_file=hk_config.get('persist_file', 'homekit.state'),
            on_fan_command=lambda room, service, data: handle_service_call('fan', service, [f'fan.{room}_fan'], data),
            on_light_command=lambda room, service, data: handle_service_call('light', service, [f'light.{room}_fan_light'], data),
        )
        homekit_bridge.start()

    init_entities()

    ws_url = HA_URL.replace('https://', 'wss://').replace('http://', 'ws://') + '/api/websocket'
    logging.info(f"Connecting to {ws_url}")

    while True:
        try:
            ws = websocket.WebSocketApp(
                ws_url,
                on_message=on_ws_message,
                on_error=on_ws_error,
                on_close=on_ws_close,
            )
            ws.run_forever()
        except Exception as e:
            logging.error(f"WebSocket connection failed: {e}")
        logging.info("Reconnecting in 10 seconds...")
        time.sleep(10)


if __name__ == '__main__':
    main()
