// What SDL3 makes of the pads on the machine, for the identity's spike:
// each pad as it is added, opened as a gamepad, rumbled once and asked
// for its gyro, and every button, touchpad and sensor event for a while.
// One line each on stdout, starting SDL.
#include <SDL3/SDL.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    int ms = argc > 1 ? atoi(argv[1]) : 4000;
    SDL_SetHint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, "1");
    if (!SDL_Init(SDL_INIT_GAMEPAD)) {
        printf("SDL init-failed %s\n", SDL_GetError());
        return 1;
    }
    printf("SDL version %d hidapi=%s\n", SDL_GetVersion(), SDL_GetHint(SDL_HINT_JOYSTICK_HIDAPI) ? SDL_GetHint(SDL_HINT_JOYSTICK_HIDAPI) : "(default)");
    Uint64 end = SDL_GetTicks() + (Uint64)ms;
    int sensors = 0, axes = 0;
    while (SDL_GetTicks() < end) {
        SDL_Event e;
        while (SDL_PollEvent(&e)) {
            switch (e.type) {
            case SDL_EVENT_JOYSTICK_ADDED: {
                SDL_JoystickID id = e.jdevice.which;
                char guid[33];
                SDL_GUIDToString(SDL_GetJoystickGUIDForID(id), guid, sizeof guid);
                printf("SDL added id=%u name=\"%s\" path=%s vid=%04x pid=%04x guid=%s is_gamepad=%d type=%s real=%s\n",
                       id, SDL_GetJoystickNameForID(id), SDL_GetJoystickPathForID(id),
                       SDL_GetJoystickVendorForID(id), SDL_GetJoystickProductForID(id), guid,
                       SDL_IsGamepad(id),
                       SDL_GetGamepadStringForType(SDL_GetGamepadTypeForID(id)),
                       SDL_GetGamepadStringForType(SDL_GetRealGamepadTypeForID(id)));
                SDL_Gamepad *pad = SDL_OpenGamepad(id);
                if (!pad) {
                    printf("SDL open-failed id=%u %s\n", id, SDL_GetError());
                    break;
                }
                SDL_Joystick *joy = SDL_GetGamepadJoystick(pad);
                char *mapping = SDL_GetGamepadMapping(pad);
                printf("SDL opened id=%u serial=%s buttons=%d axes=%d touchpads=%d gyro=%d accel=%d rumble=%d\n",
                       id, SDL_GetGamepadSerial(pad) ? SDL_GetGamepadSerial(pad) : "(none)",
                       SDL_GetNumJoystickButtons(joy), SDL_GetNumJoystickAxes(joy),
                       SDL_GetNumGamepadTouchpads(pad),
                       SDL_GamepadHasSensor(pad, SDL_SENSOR_GYRO), SDL_GamepadHasSensor(pad, SDL_SENSOR_ACCEL),
                       SDL_GetBooleanProperty(SDL_GetGamepadProperties(pad), SDL_PROP_GAMEPAD_CAP_RUMBLE_BOOLEAN, false));
                printf("SDL mapping id=%u %s\n", id, mapping ? mapping : "(none)");
                SDL_free(mapping);
                printf("SDL rumble id=%u ok=%d\n", id, SDL_RumbleGamepad(pad, 0x8000, 0x4000, 300));
                printf("SDL gyro-on id=%u ok=%d\n", id, SDL_SetGamepadSensorEnabled(pad, SDL_SENSOR_GYRO, true));
                break;
            }
            case SDL_EVENT_JOYSTICK_REMOVED:
                printf("SDL removed id=%u\n", e.jdevice.which);
                break;
            case SDL_EVENT_GAMEPAD_BUTTON_DOWN:
            case SDL_EVENT_GAMEPAD_BUTTON_UP:
                printf("SDL button id=%u %s %s\n", e.gbutton.which,
                       SDL_GetGamepadStringForButton((SDL_GamepadButton)e.gbutton.button),
                       e.gbutton.down ? "down" : "up");
                break;
            case SDL_EVENT_JOYSTICK_BUTTON_DOWN:
                printf("SDL joybutton id=%u %d down\n", e.jbutton.which, e.jbutton.button);
                break;
            case SDL_EVENT_GAMEPAD_AXIS_MOTION:
                if (axes++ < 8) {
                    printf("SDL axis id=%u %s %d\n", e.gaxis.which,
                           SDL_GetGamepadStringForAxis((SDL_GamepadAxis)e.gaxis.axis), e.gaxis.value);
                }
                break;
            case SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN:
            case SDL_EVENT_GAMEPAD_TOUCHPAD_UP:
                printf("SDL touchpad id=%u pad=%d %s x=%.3f y=%.3f p=%.3f\n", e.gtouchpad.which, e.gtouchpad.touchpad,
                       e.type == SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN ? "down" : "up",
                       e.gtouchpad.x, e.gtouchpad.y, e.gtouchpad.pressure);
                break;
            case SDL_EVENT_GAMEPAD_SENSOR_UPDATE:
                if (sensors++ < 4) {
                    printf("SDL sensor id=%u kind=%d %.3f %.3f %.3f\n", e.gsensor.which, e.gsensor.sensor,
                           e.gsensor.data[0], e.gsensor.data[1], e.gsensor.data[2]);
                }
                break;
            default:
                break;
            }
            fflush(stdout);
        }
        SDL_Delay(5);
    }
    printf("SDL done sensors=%d axes=%d\n", sensors, axes);
    SDL_Quit();
    return 0;
}
