/**
 * Unified input abstraction for keyboard, pointer, gamepad, and switch input.
 *
 * Gamepad / switch mappings (documented for players and operators):
 *   - D-pad Up / Left Stick Up        -> focus previous (move focus up)
 *   - D-pad Down / Left Stick Down    -> focus next (move focus down)
 *   - D-pad Left / Left Stick Left    -> focus previous (move focus left)
 *   - D-pad Right / Left Stick Right  -> focus next (move focus right)
 *   - A (button 0) / Switch "select"  -> confirm / activate focused control
 *   - B (button 1) / Switch "back"    -> cancel / close modal / exit
 *   - Start (button 9)                -> pause / resume
 *
 * Focus is always made visible: when a gamepad or switch drives focus we add a
 * `data-input-modality="gamepad"` attribute on the document root so styles can
 * render a persistent focus ring (see globals.css `.focus-visible` rules).
 *
 * Submissions are deduplicated per action window: a single physical press can
 * only trigger one action, and repeated presses within `ACTION_WINDOW_MS` for
 * the same action key are ignored. This prevents duplicate submissions when a
 * gamepad button, keyboard key, and pointer all fire for the same intent.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

export type InputModality = 'keyboard' | 'pointer' | 'gamepad' | 'switch';

export type InputAction =
  | 'focus-next'
  | 'focus-prev'
  | 'confirm'
  | 'cancel'
  | 'pause';

export interface InputEventDetail {
  action: InputAction;
  modality: InputModality;
  /** Stable key used for deduplication, e.g. `confirm:submit`. */
  actionKey: string;
  /** The element that was focused when the action fired, if any. */
  target: Element | null;
  /** Original DOM / gamepad event, when available. */
  source?: Event | Gamepad;
}

export type InputActionHandler = (detail: InputEventDetail) => void;

/** Minimum time between two identical actions before one is dropped. */
export const ACTION_WINDOW_MS = 250;

/** Gamepad button indices (Standard Gamepad mapping). */
export const GAMEPAD_BUTTONS = {
  A: 0,
  B: 1,
  START: 9,
  DPAD_UP: 12,
  DPAD_DOWN: 13,
  DPAD_LEFT: 14,
  DPAD_RIGHT: 15,
} as const;

/** Analog stick deadzone for focus navigation. */
export const STICK_DEADZONE = 0.5;

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
  '[role="button"]:not([aria-disabled="true"])',
].join(',');

const MODALITY_ATTR = 'data-input-modality';

function setModality(modality: InputModality): void {
  if (typeof document === 'undefined') return;
  document.documentElement.setAttribute(MODALITY_ATTR, modality);
}

function getFocusable(root: ParentNode = document): HTMLElement[] {
  const nodes = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
  return nodes.filter((el) => {
    if (el.hasAttribute('disabled')) return false;
    if (el.getAttribute('aria-hidden') === 'true') return false;
    const style = window.getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden') return false;
    return el.offsetParent !== null || el === document.activeElement;
  });
}

/**
 * Move focus to the next/previous focusable element. Wraps around and keeps
 * focus inside the nearest open dialog when one is present.
 */
export function moveFocus(direction: 'next' | 'prev', from?: Element | null): HTMLElement | null {
  if (typeof document === 'undefined') return null;
  const container: ParentNode =
    (from?.closest('[role="dialog"], [aria-modal="true"]') as ParentNode | null) ?? document;
  const focusable = getFocusable(container);
  if (focusable.length === 0) return null;

  const active = (document.activeElement as HTMLElement | null) ?? null;
  const currentIndex = active ? focusable.indexOf(active) : -1;

  let nextIndex: number;
  if (currentIndex === -1) {
    nextIndex = direction === 'next' ? 0 : focusable.length - 1;
  } else if (direction === 'next') {
    nextIndex = (currentIndex + 1) % focusable.length;
  } else {
    nextIndex = (currentIndex - 1 + focusable.length) % focusable.length;
  }

  const target = focusable[nextIndex];
  target.focus({ preventScroll: false });
  return target;
}

/**
 * Deduplicator that drops repeated actions for the same key within the action
 * window. Shared across all input sources so a gamepad press and a keyboard
 * press for the same intent cannot both submit.
 */
export class ActionDeduplicator {
  private lastFired = new Map<string, number>();

  constructor(private readonly windowMs: number = ACTION_WINDOW_MS) {}

  /** Returns true if the action should fire, false if it is a duplicate. */
  shouldFire(actionKey: string, now: number = Date.now()): boolean {
    const last = this.lastFired.get(actionKey);
    if (last !== undefined && now - last < this.windowMs) {
      return false;
    }
    this.lastFired.set(actionKey, now);
    return true;
  }

  reset(actionKey?: string): void {
    if (actionKey) {
      this.lastFired.delete(actionKey);
    } else {
      this.lastFired.clear();
    }
  }
}

interface GamepadState {
  pressed: Set<number>;
  stickDirection: 'up' | 'down' | 'left' | 'right' | null;
  stickReleasedAt: number;
}

/**
 * Polls connected gamepads and translates button/stick state into input
 * actions. Runs a requestAnimationFrame loop only while at least one gamepad
 * is connected.
 */
export class GamepadNavigator {
  private rafId: number | null = null;
  private states = new Map<number, GamepadState>();
  private running = false;

  constructor(
    private readonly onAction: (detail: InputEventDetail) => void,
    private readonly dedupe: ActionDeduplicator,
  ) {}

  start(): void {
    if (this.running || typeof window === 'undefined') return;
    if (!('getGamepads' in navigator)) return;
    this.running = true;
    const loop = () => {
      if (!this.running) return;
      this.poll();
      this.rafId = window.requestAnimationFrame(loop);
    };
    this.rafId = window.requestAnimationFrame(loop);
  }

  stop(): void {
    this.running = false;
    if (this.rafId !== null) {
      window.cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.states.clear();
  }

  private poll(): void {
    const pads = navigator.getGamepads ? navigator.getGamepads() : [];
    for (const pad of pads) {
      if (!pad) continue;
      const state = this.states.get(pad.index) ?? {
        pressed: new Set<number>(),
        stickDirection: null,
        stickReleasedAt: 0,
      };

      // Buttons: fire on the rising edge only.
      for (let i = 0; i < pad.buttons.length; i += 1) {
        const button = pad.buttons[i];
        const isPressed = button.pressed || button.value > 0.5;
        const wasPressed = state.pressed.has(i);
        if (isPressed && !wasPressed) {
          state.pressed.add(i);
          this.handleButton(i);
        } else if (!isPressed && wasPressed) {
          state.pressed.delete(i);
        }
      }

      // Left stick focus navigation with deadzone and edge detection.
      const [x = 0, y = 0] = pad.axes;
      let dir: GamepadState['stickDirection'] = null;
      if (Math.abs(x) > STICK_DEADZONE || Math.abs(y) > STICK_DEADZONE) {
        if (Math.abs(x) > Math.abs(y)) {
          dir = x > 0 ? 'right' : 'left';
        } else {
          dir = y > 0 ? 'down' : 'up';
        }
      }
      if (dir && dir !== state.stickDirection) {
        state.stickDirection = dir;
        this.handleStick(dir);
      } else if (!dir) {
        state.stickDirection = null;
      }

      this.states.set(pad.index, state);
    }
  }

  private handleButton(index: number): void {
    switch (index) {
      case GAMEPAD_BUTTONS.A:
        this.emit('confirm', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.B:
        this.emit('cancel', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.START:
        this.emit('pause', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.DPAD_UP:
        this.emit('focus-prev', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.DPAD_DOWN:
        this.emit('focus-next', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.DPAD_LEFT:
        this.emit('focus-prev', 'gamepad');
        break;
      case GAMEPAD_BUTTONS.DPAD_RIGHT:
        this.emit('focus-next', 'gamepad');
        break;
      default:
        break;
    }
  }

  private handleStick(dir: NonNullable<GamepadState['stickDirection']>): void {
    if (dir === 'up' || dir === 'left') {
      this.emit('focus-prev', 'gamepad');
    } else {
      this.emit('focus-next', 'gamepad');
    }
  }

  private emit(action: InputAction, modality: InputModality): void {
    const target = typeof document !== 'undefined' ? document.activeElement : null;
    const actionKey = `${action}:${target instanceof HTMLElement ? target.id || target.tagName : 'root'}`;
    if (!this.dedupe.shouldFire(actionKey)) return;
    setModality(modality);
    this.onAction({ action, modality, actionKey, target });
  }
}

/**
 * React hook that wires keyboard, pointer, gamepad, and switch input into a
 * single action handler. Returns helpers for modals and play flows.
 *
 * @param onAction Called for every non-duplicate input action.
 * @param options.enableGamepad Poll for connected gamepads (default true).
 */
export function useUnifiedInput(
  onAction: InputActionHandler,
  options: { enableGamepad?: boolean } = {},
): {
  modality: InputModality;
  dedupe: ActionDeduplicator;
  focusNext: () => void;
  focusPrev: () => void;
} {
  const { enableGamepad = true } = options;
  const dedupeRef = useRef<ActionDeduplicator>(new ActionDeduplicator());
  const handlerRef = useRef<InputActionHandler>(onAction);
  const [modality, setModalityState] = useState<InputModality>('keyboard');

  handlerRef.current = onAction;

  const dispatch = useCallback((detail: InputEventDetail) => {
    setModalityState(detail.modality);
    handlerRef.current(detail);
  }, []);

  const focusNext = useCallback(() => {
    setModality('gamepad');
    moveFocus('next');
  }, []);

  const focusPrev = useCallback(() => {
    setModality('gamepad');
    moveFocus('prev');
  }, []);

  useEffect(() => {
    if (typeof window === 'undefined') return;

    const dedupe = dedupeRef.current;

    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const isTextEntry =
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target?.isContentEditable ?? false);

      let action: InputAction | null = null;
      if (event.key === 'Tab') {
        // Let the browser handle Tab focus order; just record modality.
        setModality('keyboard');
        return;
      }
      if (event.key === 'Escape') action = 'cancel';
      else if (event.key === 'Enter' || event.key === ' ') {
        if (isTextEntry && event.key === ' ') return;
        action = 'confirm';
      } else if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
        action = 'focus-next';
      } else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
        action = 'focus-prev';
      } else if (event.key === 'p' || event.key === 'P') {
        action = 'pause';
      }

      if (!action) return;

      const actionKey = `${action}:${target?.id || target?.tagName || 'root'}`;
      if (!dedupe.shouldFire(actionKey)) {
        event.preventDefault();
        return;
      }

      if (action === 'focus-next') {
        event.preventDefault();
        moveFocus('next');
      } else if (action === 'focus-prev') {
        event.preventDefault();
        moveFocus('prev');
      }

      setModality('keyboard');
      dispatch({ action, modality: 'keyboard', actionKey, target, source: event });
    };

    const onPointerDown = () => {
      setModality('pointer');
    };

    const onGamepadConnected = () => {
      navigatorRef.start();
    };

    const navigatorRef = new GamepadNavigator((detail) => {
      if (detail.action === 'focus-next') {
        moveFocus('next');
      } else if (detail.action === 'focus-prev') {
        moveFocus('prev');
      }
      dispatch(detail);
    }, dedupe);

    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('pointerdown', onPointerDown, { passive: true });

    if (enableGamepad) {
      window.addEventListener('gamepadconnected', onGamepadConnected);
      navigatorRef.start();
    }

    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('gamepadconnected', onGamepadConnected);
      navigatorRef.stop();
    };
  }, [dispatch, enableGamepad]);

  return { modality, dedupe: dedupeRef.current, focusNext, focusPrev };
}

/**
 * Hook for modals: traps focus, wires cancel (B / Escape) to close, and
 * ensures the modal can be reached and exited without a pointer.
 */
export function useModalInput(options: {
  open: boolean;
  onClose: () => void;
  onConfirm?: () => void;
  initialFocusRef?: React.RefObject<HTMLElement>;
}): { containerRef: React.RefObject<HTMLDivElement> } {
  const { open, onClose, onConfirm, initialFocusRef } = options;
  const containerRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<Element | null>(null);

  useUnifiedInput(
    (detail) => {
      if (!open) return;
      if (detail.action === 'cancel') {
        onClose();
      } else if (detail.action === 'confirm' && onConfirm) {
        onConfirm();
      }
    },
    { enableGamepad: open },
  );

  useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement;

    const container = containerRef.current;
    const target = initialFocusRef?.current ?? (container ? getFocusable(container)[0] : null);
    target?.focus();

    return () => {
      const prev = previousFocusRef.current;
      if (prev instanceof HTMLElement) {
        prev.focus();
      }
    };
  }, [open, initialFocusRef]);

  return { containerRef };
}

/**
 * Hook for play flows: exposes a deduplicated submit function so a gamepad
 * confirm, keyboard Enter, and pointer click cannot double-submit.
 */
export function useDedupedSubmit<T>(
  submit: (payload: T) => void | Promise<void>,
  actionKey = 'confirm:submit',
): (payload: T) => void {
  const dedupeRef = useRef<ActionDeduplicator>(new ActionDeduplicator());
  const inFlightRef = useRef(false);

  return useCallback(
    (payload: T) => {
      if (inFlightRef.current) return;
      if (!dedupeRef.current.shouldFire(actionKey)) return;
      inFlightRef.current = true;
      const result = submit(payload);
      if (result && typeof (result as Promise<void>).then === 'function') {
        (result as Promise<void>).finally(() => {
          inFlightRef.current = false;
        });
      } else {
        inFlightRef.current = false;
      }
    },
    [submit, actionKey],
  );
}