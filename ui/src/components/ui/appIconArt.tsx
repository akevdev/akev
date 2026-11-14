import type { ReactNode } from 'react';
import type { AppIconName } from './AppIcon';

/**
 * The SVG artwork for {@link AppIcon}, loaded on its own chunk (it is most of the icon's
 * weight and nothing needs it before the first paint).
 */

interface IconDef {
  render: (uid: string) => ReactNode;
}

const W = '#ffffff';

/*
 * Simplified, original geometric glyphs evoking each app (24×24 viewBox). They are kept
 * deliberately abstract — recognisable at 14–40px without reproducing trademarks exactly.
 */
export const ICON_ART: Record<AppIconName, IconDef> = {
  chrome: {
    render: () => (
      <>
        <path d="M12 12 3.34 7A10 10 0 0 1 20.66 7Z" fill="#ea4335" />
        <path d="M12 12 20.66 7A10 10 0 0 1 12 22Z" fill="#fbbc04" />
        <path d="M12 12 12 22A10 10 0 0 1 3.34 7Z" fill="#34a853" />
        <circle cx="12" cy="12" r="5" fill={W} />
        <circle cx="12" cy="12" r="3.9" fill="#4285f4" />
      </>
    ),
  },
  chromium: {
    render: () => (
      <>
        <path d="M12 12 3.34 7A10 10 0 0 1 20.66 7Z" fill="#2f6fe0" />
        <path d="M12 12 20.66 7A10 10 0 0 1 12 22Z" fill="#8ab4f8" />
        <path d="M12 12 12 22A10 10 0 0 1 3.34 7Z" fill="#5a95f5" />
        <circle cx="12" cy="12" r="5" fill={W} />
        <circle cx="12" cy="12" r="3.9" fill="#2f6fe0" />
      </>
    ),
  },
  firefox: {
    // The official Firefox logo (mozilla/protocol-assets, logos/firefox/browser/logo.svg,
    // MPL-2.0; Firefox is a trademark of the Mozilla Foundation), shown to identify the browser.
    render: (uid) => (
      <>
        <g transform="translate(1.25 1.25) scale(0.041992188)">
          <defs>
            <radialGradient id={`${uid}ff-b`} cx="87.4%" cy="-12.9%" r="128%" gradientTransform="matrix(.8 0 0 1 .178 .129)">
              <stop offset=".13" stopColor="#ffbd4f" />
              <stop offset=".28" stopColor="#ff980e" />
              <stop offset=".47" stopColor="#ff3750" />
              <stop offset=".78" stopColor="#eb0878" />
              <stop offset=".86" stopColor="#e50080" />
            </radialGradient>
            <radialGradient id={`${uid}ff-c`} cx="49%" cy="40%" r="128%" gradientTransform="matrix(.82 0 0 1 .088 0)">
              <stop offset=".3" stopColor="#960e18" />
              <stop offset=".35" stopColor="#b11927" stopOpacity=".74" />
              <stop offset=".43" stopColor="#db293d" stopOpacity=".34" />
              <stop offset=".5" stopColor="#f5334b" stopOpacity=".09" />
              <stop offset=".53" stopColor="#ff3750" stopOpacity="0" />
            </radialGradient>
            <radialGradient id={`${uid}ff-d`} cx="48%" cy="-12%" r="140%">
              <stop offset=".13" stopColor="#fff44f" />
              <stop offset=".53" stopColor="#ff980e" />
            </radialGradient>
            <radialGradient id={`${uid}ff-e`} cx="22.76%" cy="110.11%" r="100%">
              <stop offset=".35" stopColor="#3a8ee6" />
              <stop offset=".67" stopColor="#9059ff" />
              <stop offset="1" stopColor="#c139e6" />
            </radialGradient>
            <radialGradient id={`${uid}ff-f`} cx="52%" cy="33%" r="59%" gradientTransform="scale(.9 1)">
              <stop offset=".21" stopColor="#9059ff" stopOpacity="0" />
              <stop offset=".97" stopColor="#6e008b" stopOpacity=".6" />
            </radialGradient>
            <radialGradient id={`${uid}ff-g`} cx="210%" cy="-100%" r="290%">
              <stop offset=".1" stopColor="#ffe226" />
              <stop offset=".79" stopColor="#ff7139" />
            </radialGradient>
            <radialGradient id={`${uid}ff-h`} cx="84%" cy="-41%" r="180%">
              <stop offset=".11" stopColor="#fff44f" />
              <stop offset=".46" stopColor="#ff980e" />
              <stop offset=".72" stopColor="#ff3647" />
              <stop offset=".9" stopColor="#e31587" />
            </radialGradient>
            <radialGradient id={`${uid}ff-i`} cx="16.1%" cy="-18.6%" r="348.8%" gradientTransform="matrix(.10453 .46743 -.99452 .04913 -.046 -.255)">
              <stop offset="0" stopColor="#fff44f" />
              <stop offset=".3" stopColor="#ff980e" />
              <stop offset=".57" stopColor="#ff3647" />
              <stop offset=".74" stopColor="#e31587" />
            </radialGradient>
            <radialGradient id={`${uid}ff-j`} cx="18.9%" cy="-42.5%" r="238.4%">
              <stop offset=".14" stopColor="#fff44f" />
              <stop offset=".48" stopColor="#ff980e" />
              <stop offset=".66" stopColor="#ff3647" />
              <stop offset=".9" stopColor="#e31587" />
            </radialGradient>
            <radialGradient id={`${uid}ff-k`} cx="159.3%" cy="-44.72%" r="313.1%">
              <stop offset=".09" stopColor="#fff44f" />
              <stop offset=".63" stopColor="#ff980e" />
            </radialGradient>
            <linearGradient id={`${uid}ff-a`} x1="87.25%" x2="9.4%" y1="15.5%" y2="93.1%">
              <stop offset=".05" stopColor="#fff44f" />
              <stop offset=".37" stopColor="#ff980e" />
              <stop offset=".53" stopColor="#ff3647" />
              <stop offset=".7" stopColor="#e31587" />
            </linearGradient>
            <linearGradient id={`${uid}ff-l`} x1="80%" x2="18%" y1="14%" y2="84%">
              <stop offset=".17" stopColor="#fff44f" stopOpacity=".8" />
              <stop offset=".6" stopColor="#fff44f" stopOpacity="0" />
            </linearGradient>
          </defs>
          <path fill={`url(#${uid}ff-a)`} d="M478.711 166.353c-10.445-25.124-31.6-52.248-48.212-60.821 13.52 26.505 21.345 53.093 24.335 72.936 0 .039.015.136.047.4C427.706 111.135 381.627 83.823 344 24.355c-1.9-3.007-3.805-6.022-5.661-9.2a74 74 0 0 1-2.646-4.972A43.7 43.7 0 0 1 332.1.677a.626.626 0 0 0-.546-.644.8.8 0 0 0-.451 0c-.034.012-.084.051-.12.065-.053.021-.12.069-.176.1.027-.036.083-.117.1-.136-60.37 35.356-80.85 100.761-82.732 133.484a120.25 120.25 0 0 0-66.142 25.488 71 71 0 0 0-6.225-4.7 111.34 111.34 0 0 1-.674-58.732c-24.688 11.241-43.89 29.01-57.85 44.7h-.111c-9.527-12.067-8.855-51.873-8.312-60.184-.114-.515-7.107 3.63-8.023 4.255a175 175 0 0 0-23.486 20.12 210.5 210.5 0 0 0-22.442 26.913c0 .012-.007.026-.011.038 0-.013.007-.026.011-.038a202.8 202.8 0 0 0-32.247 72.805c-.115.521-.212 1.061-.324 1.586-.452 2.116-2.08 12.7-2.365 15-.022.177-.032.347-.053.524a229 229 0 0 0-3.9 33.157c0 .41-.025.816-.025 1.227C16 388.418 123.6 496 256.324 496c118.865 0 217.56-86.288 236.882-199.63.407-3.076.733-6.168 1.092-9.271 4.777-41.21-.53-84.525-15.587-120.746M201.716 354.447c1.124.537 2.18 1.124 3.334 1.639.048.033.114.07.163.1a126 126 0 0 1-3.497-1.739M454.9 178.927l-.032-.233c.012.085.027.174.04.259z" />
          <path fill={`url(#${uid}ff-b)`} d="M478.711 166.353c-10.445-25.124-31.6-52.248-48.212-60.821 13.52 26.505 21.345 53.093 24.335 72.936 0-.058.011.048.036.226l.04.259c22.675 61.47 10.322 123.978-7.479 162.175-27.539 59.1-94.215 119.67-198.576 116.716C136.1 454.651 36.766 370.988 18.223 261.41c-3.379-17.28 0-26.054 1.7-40.084-2.071 10.816-2.86 13.94-3.9 33.157 0 .41-.025.816-.025 1.227C16 388.418 123.6 496 256.324 496c118.865 0 217.56-86.288 236.882-199.63.407-3.076.733-6.168 1.092-9.271 4.777-41.21-.53-84.525-15.587-120.746" />
          <path fill={`url(#${uid}ff-c)`} d="M478.711 166.353c-10.445-25.124-31.6-52.248-48.212-60.821 13.52 26.505 21.345 53.093 24.335 72.936 0-.058.011.048.036.226l.04.259c22.675 61.47 10.322 123.978-7.479 162.175-27.539 59.1-94.215 119.67-198.576 116.716C136.1 454.651 36.766 370.988 18.223 261.41c-3.379-17.28 0-26.054 1.7-40.084-2.071 10.816-2.86 13.94-3.9 33.157 0 .41-.025.816-.025 1.227C16 388.418 123.6 496 256.324 496c118.865 0 217.56-86.288 236.882-199.63.407-3.076.733-6.168 1.092-9.271 4.777-41.21-.53-84.525-15.587-120.746" />
          <path fill={`url(#${uid}ff-d)`} d="M361.922 194.6c.524.368 1 .734 1.493 1.1a130.7 130.7 0 0 0-22.31-29.112C266.4 91.892 321.516 4.626 330.811.194c.027-.036.083-.117.1-.136-60.37 35.356-80.85 100.761-82.732 133.484 2.8-.194 5.592-.429 8.442-.429 45.051 0 84.289 24.77 105.301 61.487" />
          <path fill={`url(#${uid}ff-e)`} d="M256.772 209.514c-.393 5.978-21.514 26.593-28.9 26.593-68.339 0-79.432 41.335-79.432 41.335 3.027 34.81 27.261 63.475 56.611 78.643 1.339.692 2.694 1.317 4.05 1.935a133 133 0 0 0 7.059 2.886 106.7 106.7 0 0 0 31.271 6.031c119.78 5.618 142.986-143.194 56.545-186.408 22.137-3.85 45.115 5.053 57.947 14.067-21.012-36.714-60.25-61.484-105.3-61.484-2.85 0-5.641.235-8.442.429a120.25 120.25 0 0 0-66.142 25.488c3.664 3.1 7.8 7.244 16.514 15.828 16.302 16.067 58.13 32.705 58.219 34.657" />
          <path fill={`url(#${uid}ff-f)`} d="M256.772 209.514c-.393 5.978-21.514 26.593-28.9 26.593-68.339 0-79.432 41.335-79.432 41.335 3.027 34.81 27.261 63.475 56.611 78.643 1.339.692 2.694 1.317 4.05 1.935a133 133 0 0 0 7.059 2.886 106.7 106.7 0 0 0 31.271 6.031c119.78 5.618 142.986-143.194 56.545-186.408 22.137-3.85 45.115 5.053 57.947 14.067-21.012-36.714-60.25-61.484-105.3-61.484-2.85 0-5.641.235-8.442.429a120.25 120.25 0 0 0-66.142 25.488c3.664 3.1 7.8 7.244 16.514 15.828 16.302 16.067 58.13 32.705 58.219 34.657" />
          <path fill={`url(#${uid}ff-g)`} d="M170.829 151.036a244 244 0 0 1 4.981 3.3 111.34 111.34 0 0 1-.674-58.732c-24.688 11.241-43.89 29.01-57.85 44.7 1.155-.033 36.014-.66 53.543 10.732" />
          <path fill={`url(#${uid}ff-h)`} d="M18.223 261.41C36.766 370.988 136.1 454.651 248.855 457.844c104.361 2.954 171.037-57.62 198.576-116.716 17.8-38.2 30.154-100.7 7.479-162.175l-.008-.026-.032-.233c-.025-.178-.04-.284-.036-.226 0 .039.015.136.047.4 8.524 55.661-19.79 109.584-64.051 146.044l-.133.313c-86.245 70.223-168.774 42.368-185.484 30.966a144 144 0 0 1-3.5-1.743c-50.282-24.029-71.054-69.838-66.6-109.124-42.457 0-56.934-35.809-56.934-35.809s38.119-27.179 88.358-3.541c46.53 21.893 90.228 3.543 90.233 3.541-.089-1.952-41.917-18.59-58.223-34.656-8.713-8.584-12.85-12.723-16.514-15.828a71 71 0 0 0-6.225-4.7 283 283 0 0 0-4.981-3.3c-17.528-11.392-52.388-10.765-53.543-10.735h-.111c-9.527-12.067-8.855-51.873-8.312-60.184-.114-.515-7.107 3.63-8.023 4.255a175 175 0 0 0-23.486 20.12 210.5 210.5 0 0 0-22.442 26.919c0 .012-.007.026-.011.038 0-.013.007-.026.011-.038a202.8 202.8 0 0 0-32.247 72.805c-.115.521-8.65 37.842-4.44 57.199" />
          <path fill={`url(#${uid}ff-i)`} d="M341.105 166.587a130.7 130.7 0 0 1 22.31 29.112c1.323.994 2.559 1.985 3.608 2.952 54.482 50.2 25.936 121.2 23.807 126.26 44.261-36.46 72.575-90.383 64.051-146.044C427.706 111.135 381.627 83.823 344 24.355c-1.9-3.007-3.805-6.022-5.661-9.2a74 74 0 0 1-2.646-4.972A43.7 43.7 0 0 1 332.1.677a.626.626 0 0 0-.546-.644.8.8 0 0 0-.451 0c-.034.012-.084.051-.12.065-.053.021-.12.069-.176.1-9.291 4.428-64.407 91.694 10.298 166.389" />
          <path fill={`url(#${uid}ff-j)`} d="M367.023 198.651c-1.049-.967-2.285-1.958-3.608-2.952-.489-.368-.969-.734-1.493-1.1-12.832-9.014-35.81-17.917-57.947-14.067 86.441 43.214 63.235 192.026-56.545 186.408a106.7 106.7 0 0 1-31.271-6.031 135 135 0 0 1-7.059-2.886c-1.356-.618-2.711-1.243-4.05-1.935.048.033.114.07.163.1 16.71 11.4 99.239 39.257 185.484-30.966l.133-.313c2.129-5.054 30.675-76.057-23.807-126.258" />
          <path fill={`url(#${uid}ff-k)`} d="M148.439 277.443s11.093-41.335 79.432-41.335c7.388 0 28.509-20.615 28.9-26.593s-43.7 18.352-90.233-3.541c-50.239-23.638-88.358 3.541-88.358 3.541s14.477 35.809 56.934 35.809c-4.453 39.286 16.319 85.1 66.6 109.124 1.124.537 2.18 1.124 3.334 1.639-29.348-15.169-53.582-43.834-56.609-78.644" />
          <path fill={`url(#${uid}ff-l)`} d="M478.711 166.353c-10.445-25.124-31.6-52.248-48.212-60.821 13.52 26.505 21.345 53.093 24.335 72.936 0 .039.015.136.047.4C427.706 111.135 381.627 83.823 344 24.355c-1.9-3.007-3.805-6.022-5.661-9.2a74 74 0 0 1-2.646-4.972A43.7 43.7 0 0 1 332.1.677a.626.626 0 0 0-.546-.644.8.8 0 0 0-.451 0c-.034.012-.084.051-.12.065-.053.021-.12.069-.176.1.027-.036.083-.117.1-.136-60.37 35.356-80.85 100.761-82.732 133.484 2.8-.194 5.592-.429 8.442-.429 45.053 0 84.291 24.77 105.3 61.484-12.832-9.014-35.81-17.917-57.947-14.067 86.441 43.214 63.235 192.026-56.545 186.408a106.7 106.7 0 0 1-31.271-6.031 135 135 0 0 1-7.059-2.886c-1.356-.618-2.711-1.243-4.05-1.935.048.033.114.07.163.1a144 144 0 0 1-3.5-1.743c1.124.537 2.18 1.124 3.334 1.639-29.35-15.168-53.584-43.833-56.611-78.643 0 0 11.093-41.335 79.432-41.335 7.388 0 28.509-20.615 28.9-26.593-.089-1.952-41.917-18.59-58.223-34.656-8.713-8.584-12.85-12.723-16.514-15.828a71 71 0 0 0-6.225-4.7 111.34 111.34 0 0 1-.674-58.732c-24.688 11.241-43.89 29.01-57.85 44.7h-.111c-9.527-12.067-8.855-51.873-8.312-60.184-.114-.515-7.107 3.63-8.023 4.255a175 175 0 0 0-23.486 20.12 210.5 210.5 0 0 0-22.435 26.916c0 .012-.007.026-.011.038 0-.013.007-.026.011-.038a202.8 202.8 0 0 0-32.247 72.805c-.115.521-.212 1.061-.324 1.586-.452 2.116-2.486 12.853-2.77 15.156-.022.177.021-.176 0 0a280 280 0 0 0-3.544 33.53c0 .41-.025.816-.025 1.227C16 388.418 123.6 496 256.324 496c118.865 0 217.56-86.288 236.882-199.63.407-3.076.733-6.168 1.092-9.271 4.777-41.21-.53-84.525-15.587-120.746m-23.841 12.341.04.259-.008-.026z" />
        </g>
      </>
    ),
  },
  edge: {
    // The Microsoft Edge logo (2019; Wikimedia Commons, MIT). Edge is a Microsoft trademark.
    render: (uid) => (
      <>
        <g transform="translate(1.5 1.5) scale(0.0820312)">
          <defs>
            <radialGradient id={`${uid}ed-b`} cx="161.8" cy="68.9" r="95.4" gradientTransform="matrix(1 0 0 -0.9 0 248.8)" gradientUnits="userSpaceOnUse">
              <stop offset=".7" stopOpacity="0" />
              <stop offset=".9" stopOpacity=".5" />
              <stop offset="1" />
            </radialGradient>
            <radialGradient id={`${uid}ed-d`} cx="-340.3" cy="63" r="143.2" gradientTransform="matrix(0.1 -1 -0.8 -0.1 176.6 -125.4)" gradientUnits="userSpaceOnUse">
              <stop offset=".8" stopOpacity="0" />
              <stop offset=".9" stopOpacity=".5" />
              <stop offset="1" />
            </radialGradient>
            <radialGradient id={`${uid}ed-e`} cx="113.4" cy="570.2" r="202.4" gradientTransform="matrix(0 1 2.1 0.1 -1179.5 -106.7)" gradientUnits="userSpaceOnUse">
              <stop offset="0" stopColor="#35c1f1" />
              <stop offset=".1" stopColor="#34c1ed" />
              <stop offset=".2" stopColor="#2fc2df" />
              <stop offset=".3" stopColor="#2bc3d2" />
              <stop offset=".7" stopColor="#36c752" />
            </radialGradient>
            <radialGradient id={`${uid}ed-f`} cx="376.5" cy="568" r="97.3" gradientTransform="matrix(0.3 1 0.8 -0.2 -303.8 -148.5)" gradientUnits="userSpaceOnUse">
              <stop offset="0" stopColor="#66eb6e" />
              <stop offset="1" stopColor="#66eb6e" stopOpacity="0" />
            </radialGradient>
            <linearGradient id={`${uid}ed-a`} x1="63.3" y1="84" x2="241.7" y2="84" gradientTransform="matrix(1 0 0 -1 0 266)" gradientUnits="userSpaceOnUse">
              <stop offset="0" stopColor="#0c59a4" />
              <stop offset="1" stopColor="#114a8b" />
            </linearGradient>
            <linearGradient id={`${uid}ed-c`} x1="157.3" y1="161.4" x2="46" y2="40.1" gradientTransform="matrix(1 0 0 -1 0 266)" gradientUnits="userSpaceOnUse">
              <stop offset="0" stopColor="#1b9de2" />
              <stop offset=".2" stopColor="#1595df" />
              <stop offset=".7" stopColor="#0680d7" />
              <stop offset="1" stopColor="#0078d4" />
            </linearGradient>
          </defs>
          <path d="M235.7 195.5a93.7 93.7 0 0 1-10.6 4.7 101.9 101.9 0 0 1-35.9 6.4c-47.3 0-88.5-32.5-88.5-74.3a31.5 31.5 0 0 1 16.4-27.3c-42.8 1.8-53.8 46.4-53.8 72.5 0 74 68.1 81.4 82.8 81.4 7.9 0 19.8-2.3 27-4.6l1.3-0.4a128.3 128.3 0 0 0 66.6-52.8 4 4 0 0 0-5.3-5.6Z" transform="translate(-4.6 -5)" fill={`url(#${uid}ed-a)`} />
          <path d="M235.7 195.5a93.7 93.7 0 0 1-10.6 4.7 101.9 101.9 0 0 1-35.9 6.4c-47.3 0-88.5-32.5-88.5-74.3a31.5 31.5 0 0 1 16.4-27.3c-42.8 1.8-53.8 46.4-53.8 72.5 0 74 68.1 81.4 82.8 81.4 7.9 0 19.8-2.3 27-4.6l1.3-0.4a128.3 128.3 0 0 0 66.6-52.8 4 4 0 0 0-5.3-5.6Z" transform="translate(-4.6 -5)" opacity=".35" fill={`url(#${uid}ed-b)`} />
          <path d="M110.3 246.3A79.2 79.2 0 0 1 87.6 225a80.7 80.7 0 0 1 29.5-120c3.2-1.5 8.5-4.1 15.6-4a32.4 32.4 0 0 1 25.7 13 31.9 31.9 0 0 1 6.3 18.7c0-0.2 24.5-79.6-80-79.6-43.9 0-80 41.6-80 78.2a130.2 130.2 0 0 0 12.1 56 128 128 0 0 0 156.4 67 75.5 75.5 0 0 1-62.8-8Z" transform="translate(-4.6 -5)" fill={`url(#${uid}ed-c)`} />
          <path d="M110.3 246.3A79.2 79.2 0 0 1 87.6 225a80.7 80.7 0 0 1 29.5-120c3.2-1.5 8.5-4.1 15.6-4a32.4 32.4 0 0 1 25.7 13 31.9 31.9 0 0 1 6.3 18.7c0-0.2 24.5-79.6-80-79.6-43.9 0-80 41.6-80 78.2a130.2 130.2 0 0 0 12.1 56 128 128 0 0 0 156.4 67 75.5 75.5 0 0 1-62.8-8Z" transform="translate(-4.6 -5)" opacity=".41" fill={`url(#${uid}ed-d)`} />
          <path d="M157 153.8c-0.9 1-3.4 2.5-3.4 5.6 0 2.6 1.7 5.2 4.8 7.3 14.3 10 41.4 8.6 41.5 8.6a59.6 59.6 0 0 0 30.3-8.3 61.4 61.4 0 0 0 30.4-52.9c0.3-22.4-8-37.3-11.3-43.9C228 28.8 182.3 5 132.6 5a128 128 0 0 0-128 126.2c0.5-36.5 36.8-66 80-66 3.5 0 23.5 0.3 42 10a72.6 72.6 0 0 1 30.9 29.3c6.1 10.6 7.2 24.1 7.2 29.5s-2.7 13.3-7.8 19.9Z" transform="translate(-4.6 -5)" fill={`url(#${uid}ed-e)`} />
          <path d="M157 153.8c-0.9 1-3.4 2.5-3.4 5.6 0 2.6 1.7 5.2 4.8 7.3 14.3 10 41.4 8.6 41.5 8.6a59.6 59.6 0 0 0 30.3-8.3 61.4 61.4 0 0 0 30.4-52.9c0.3-22.4-8-37.3-11.3-43.9C228 28.8 182.3 5 132.6 5a128 128 0 0 0-128 126.2c0.5-36.5 36.8-66 80-66 3.5 0 23.5 0.3 42 10a72.6 72.6 0 0 1 30.9 29.3c6.1 10.6 7.2 24.1 7.2 29.5s-2.7 13.3-7.8 19.9Z" transform="translate(-4.6 -5)" fill={`url(#${uid}ed-f)`} />
        </g>
      </>
    ),
  },
  brave: {
    // The Brave lion (Brave Software media assets via Wikimedia Commons, MPL-2.0). Brave is a
    // trademark of Brave Software, Inc.
    render: (uid) => (
      <>
        <g transform="translate(2.8 1.2) scale(0.0839844)">
          <defs>
            <linearGradient x1="0%" y1="50.7055163%" x2="100%" y2="50.7055163%" id={`${uid}br-linearGradient-1`}>
              <stop stopColor="#FF5500" offset="0%" />
              <stop stopColor="#FF5500" offset="40.9877232%" />
              <stop stopColor="#FF2000" offset="58.1981215%" />
              <stop stopColor="#FF2000" offset="100%" />
            </linearGradient>
            <linearGradient x1="2.1484375%" y1="50.7055163%" x2="100%" y2="50.7055163%" id={`${uid}br-linearGradient-2`}>
              <stop stopColor="#FF452A" offset="0%" />
              <stop stopColor="#FF2000" offset="100%" />
            </linearGradient>
            <path d="M170.3,25.3 L148,0 L108.8,0 L69.6,0 L47.3,25.3 C47.3,25.3 27.7,19.9 18.5,29.1 C18.5,29.1 44.6,26.8 53.6,41.4 C53.6,41.4 77.8,46 81.1,46 C84.3,46 91.4,43.3 97.9,41.1 C104.4,39 108.8,38.9 108.8,38.9 C108.8,38.9 113.2,39 119.7,41.1 C126.2,43.3 133.3,46 136.5,46 C139.8,46 164,41.4 164,41.4 C173,26.8 199.1,29.1 199.1,29.1 C189.9,19.9 170.3,25.3 170.3,25.3" id={`${uid}br-path-3`} />
          </defs>
          <g id={`${uid}br-starting-collection`} stroke="none" strokeWidth="1" fill="none" fillRule="evenodd">
            <g id={`${uid}br-Build-Icons`} transform="translate(-70, -350)">
              <g id={`${uid}br-build-icons/Stable`} transform="translate(50, 350)">
                <g id={`${uid}br-Logo`} transform="translate(20.1, 0)">
                  <path d="M210,61.3 L216,46.6 C216,46.6 208.4,38.4 199.1,29.1 C189.9,19.9 170.3,25.3 170.3,25.3 L148,0 L108.8,0 L69.6,0 L47.3,25.3 C47.3,25.3 27.7,19.9 18.5,29.1 C9.2,38.4 1.6,46.6 1.6,46.6 L7.6,61.3 L0,83.1 C0,83.1 22.4,168 25,178.4 C30.2,198.8 33.7,206.7 48.4,217.1 C63.1,227.4 89.8,245.4 94.1,248.1 C98.5,250.8 103.9,255.5 108.8,255.5 C113.7,255.5 119.1,250.8 123.5,248.1 C127.8,245.4 154.5,227.4 169.2,217.1 C183.9,206.7 187.4,198.8 192.6,178.4 C195.2,168 217.6,83.1 217.6,83.1 L210,61.3 Z" id={`${uid}br-Head`} fill={`url(#${uid}br-linearGradient-1)`} />
                  <path d="M164,41.4 C164,41.4 192.7,76.1 192.7,83.5 C192.7,91 189.1,92.9 185.5,96.8 C181.8,100.6 166,117.5 164,119.7 C161.9,121.8 157.7,125.1 160.2,131 C162.7,136.9 166.3,144.4 162.3,152 C158.2,159.7 151.2,164.8 146.7,163.9 C142.2,163.1 131.6,157.6 127.8,155 C123.9,152.5 111.6,142.4 111.6,138.5 C111.6,134.6 124.3,127.6 126.7,126.1 C129,124.5 139.7,118.3 139.9,115.9 C140.1,113.5 140.1,112.8 136.9,106.9 C133.7,100.9 128.1,93 129,87.7 C129.9,82.5 139.1,79.7 145.7,77.2 C152.2,74.8 164.8,70.1 166.4,69.4 C168,68.6 167.6,67.9 162.8,67.5 C158,67 144.5,65.2 138.4,66.9 C132.3,68.6 121.9,71.2 121,72.6 C120.2,74 119.4,74 120.3,78.8 C121.2,83.5 125.6,106.2 126.1,110.3 C126.5,114.3 127.3,117 123,118 C118.6,119 111.3,120.7 108.8,120.7 C106.3,120.7 99,119 94.6,118 C90.3,117 91.1,114.3 91.5,110.3 C92,106.2 96.4,83.5 97.3,78.8 C98.2,74 97.4,74 96.6,72.6 C95.7,71.2 85.3,68.6 79.2,66.9 C73.1,65.2 59.6,67 54.8,67.5 C50,67.9 49.6,68.6 51.2,69.4 C52.8,70.1 65.4,74.8 71.9,77.2 C78.5,79.7 87.7,82.5 88.6,87.7 C89.5,93 83.9,100.9 80.7,106.9 C77.5,112.8 77.5,113.5 77.7,115.9 C77.9,118.3 88.6,124.5 90.9,126.1 C93.3,127.6 106,134.6 106,138.5 C106,142.4 93.7,152.5 89.8,155 C86,157.6 75.4,163.1 70.9,163.9 C66.4,164.8 59.4,159.7 55.3,152 C51.3,144.4 54.9,136.9 57.4,131 C59.9,125.1 55.7,121.8 53.6,119.7 C51.6,117.5 35.8,100.6 32.1,96.8 C28.5,92.9 24.9,91 24.9,83.5 C24.9,76.1 53.6,41.4 53.6,41.4 C53.6,41.4 77.8,46 81.1,46 C84.3,46 91.4,43.3 97.9,41.1 C104.4,39 108.8,38.9 108.8,38.9 C108.8,38.9 113.2,39 119.7,41.1 C126.2,43.3 133.3,46 136.5,46 C139.8,46 164,41.4 164,41.4 Z M142.5,174.2 C144.3,175.3 143.2,177.4 141.6,178.6 C140,179.7 118.2,196.6 116.1,198.4 C114,200.3 110.9,203.4 108.8,203.4 C106.7,203.4 103.6,200.3 101.5,198.4 C99.4,196.6 77.6,179.7 76,178.6 C74.4,177.4 73.3,175.3 75.1,174.2 C76.9,173.1 82.4,170.3 90.1,166.3 C97.8,162.4 107.3,159 108.8,159 C110.3,159 119.8,162.4 127.5,166.3 C135.2,170.3 140.7,173.1 142.5,174.2 Z" id={`${uid}br-Face`} fill="#FFFFFF" />
                  <mask id={`${uid}br-mask-4`} fill="white">
                    <use href={`#${uid}br-path-3`} />
                  </mask>
                  <use id={`${uid}br-Top-Head`} fill={`url(#${uid}br-linearGradient-2)`} href={`#${uid}br-path-3`} />
                </g>
              </g>
            </g>
          </g>
        </g>
      </>
    ),
  },
  opera: {
    render: () => (
      <path
        fillRule="evenodd"
        d="M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20Zm0 3.2c2.1 0 3.6 3 3.6 6.8s-1.5 6.8-3.6 6.8-3.6-3-3.6-6.8 1.5-6.8 3.6-6.8Z"
        fill="#ff1b2d"
      />
    ),
  },
  vivaldi: {
    // The Vivaldi logo (Vivaldi Technologies press kit via Wikimedia Commons, CC BY 4.0;
    // scaled for icon use). Vivaldi is a trademark of Vivaldi Technologies.
    render: (uid) => (
      <>
        <g transform="translate(2 2) scale(0.0438596)">
          <defs>
            <linearGradient id={`${uid}vi-a`} x1="21.587%" x2="76.116%" y1="5.709%" y2="100.496%">
              <stop stopOpacity=".2" offset="0%" />
              <stop stopOpacity=".05" offset="79.08%" />
            </linearGradient>
          </defs>
          <g fill="none">
            <path fill="#EF3939" d="M228 454.3c99.7 0 155.1 0 191.4-36.1 36.2-36.1 36.2-91.3 36.2-190.7 0-99.4 0-154.6-36.2-190.7C383.1 0.8 327.7 0.8 228 0.8S72.9 0.8 36.6 36.9C0.4 73 0.4 128.2 0.4 227.6c0 99.4 0 154.6 36.2 190.7 36.3 36 91.7 36 191.4 36z" />
            <path fill={`url(#${uid}vi-a)`} d="M376 143.7c-21.8-38.1-58.3-67.8-104.2-80.1C180.7 39.3 87.1 93.1 62.7 183.8c-12.3 45.6-4.7 91.9 17.5 129.7 0.3 0.5 0.6 1.1 1 1.6l80.2 138.5c13.3 0.4 27.7 0.5 43.2 0.5H227.2c44.3 0 79.9 0 109-3.2 36.3-4 62.3-12.9 82.4-32.9 29.3-29.2 34.9-71 36-138.7-46.8-80.8-78.5-135.5-78.6-135.6z" />
            <path fill="#FFF" d="M347.8 107.6c-66.5-66.4-174.4-66.4-241 0-66.5 66.4-66.5 174 0 240.3 66.5 66.4 174.4 66.4 241 0s66.6-174 0-240.3zm-10.2 78.1c-28.1 48.7-56.2 97.4-84.3 146.2-5.2 9.1-12.8 14.5-23.2 15.3-11.6 0.8-20.8-4.1-26.7-14.1-17.8-30.5-35.4-61.2-53-91.8-10.7-18.6-21.5-37.2-32.2-55.9-10.8-18.8 1.4-41.7 23-42.8 11.4-0.6 20.2 4.7 26 14.6 7.9 13.5 15.7 27.2 23.6 40.8 5.7 9.8 11.2 19.7 17 29.3 8.4 14.1 20.8 22 37.3 23 23.3 1.4 45-15.5 47.8-40.2 0.2-1.8 0.3-3.7 0.4-4.6-0.1-8-1.6-14.8-4.8-21.1-8.7-17.4 0.6-36.9 19.5-41.1 15.4-3.4 31.4 7.9 33.4 23.5 1 6.7-0.4 13-3.8 18.9z" />
          </g>
        </g>
      </>
    ),
  },
  safari: {
    render: (uid) => (
      <>
        <defs>
          <linearGradient id={`${uid}sf`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor="#3bb5ff" />
            <stop offset="1" stopColor="#1468e6" />
          </linearGradient>
        </defs>
        <circle cx="12" cy="12" r="10" fill={`url(#${uid}sf)`} />
        <circle cx="12" cy="12" r="8" fill="none" stroke={W} strokeOpacity="0.55" strokeWidth="1.2" strokeDasharray="0.6 1.9" />
        <path d="M16.8 7.2 13.1 13.1 10.9 10.9Z" fill="#ff3b30" />
        <path d="M7.2 16.8 10.9 10.9 13.1 13.1Z" fill={W} />
      </>
    ),
  },
  arc: {
    render: () => (
      <>
        <rect x="2" y="2" width="20" height="20" rx="6" fill="#ff536a" />
        <path d="M7 16.5 11.2 7.6a.9.9 0 0 1 1.6 0L17 16.5M8.6 13.2c2.4-1.6 4.6-1.6 6.8 0" fill="none" stroke={W} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      </>
    ),
  },
  terminal: {
    render: () => (
      <>
        <rect x="2.5" y="4" width="19" height="16" rx="3.5" fill="#1d2129" stroke="#3a404c" strokeWidth="1" />
        <path d="m6.5 9.2 3 2.8-3 2.8" fill="none" stroke="#3ddc84" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" />
        <path d="M11.5 15.2h5" stroke="#e8eaef" strokeWidth="1.9" strokeLinecap="round" />
      </>
    ),
  },
  node: {
    render: () => (
      <>
        <path d="M12 2.2 20.5 7v10L12 21.8 3.5 17V7Z" fill="#5fa04e" />
        <path d="M9.2 15.4V8.8l5.6 6.6V8.8" fill="none" stroke={W} strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      </>
    ),
  },
  python: {
    render: () => (
      <>
        <path
          d="M11.9 2.2c-4.4 0-4.2 1.9-4.2 1.9v2h4.4v.7H5.9S3 6.5 3 11c0 4.5 2.5 4.3 2.5 4.3H7v-2.1s-.1-2.5 2.5-2.5h4.3s2.4.1 2.4-2.3V4.6s.4-2.4-4.3-2.4Zm-2.4 1.4a.8.8 0 1 1 0 1.6.8.8 0 0 1 0-1.6Z"
          fill="#3776ab"
        />
        <path
          d="M12.1 21.8c4.4 0 4.2-1.9 4.2-1.9v-2h-4.4v-.7h6.2S21 17.5 21 13c0-4.5-2.5-4.3-2.5-4.3H17v2.1s.1 2.5-2.5 2.5h-4.3s-2.4-.1-2.4 2.3v3.8s-.4 2.4 4.3 2.4Zm2.4-1.4a.8.8 0 1 1 0-1.6.8.8 0 0 1 0 1.6Z"
          fill="#ffd43b"
        />
      </>
    ),
  },
  curl: {
    render: () => (
      <>
        <circle cx="6" cy="9" r="1.6" fill="#1f9bb0" />
        <circle cx="6" cy="15.5" r="1.6" fill="#1f9bb0" />
        <path d="m10 19 4.2-14M14.6 19l4.2-14" stroke="#1f9bb0" strokeWidth="2" strokeLinecap="round" />
      </>
    ),
  },
  android: {
    render: () => (
      <>
        <path d="M3.5 17.5a8.5 8.5 0 0 1 17 0Z" fill="#3ddc84" />
        <path d="m7.4 10.6-2-3.3M16.6 10.6l2-3.3" stroke="#3ddc84" strokeWidth="1.7" strokeLinecap="round" />
        <circle cx="8.6" cy="14.4" r="1.05" fill="#0d3b22" />
        <circle cx="15.4" cy="14.4" r="1.05" fill="#0d3b22" />
      </>
    ),
  },
  apple: {
    // A generic phone (lucide "smartphone"), not the Apple logo.
    render: () => (
      <g fill="none" stroke="var(--text)" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
        <rect width="14" height="20" x="5" y="2" rx="2" ry="2" />
        <path d="M12 18h.01" />
      </g>
    ),
  },
  docker: {
    render: () => (
      <>
        <path d="M2.2 12.4h16.9c.6-1.1 1.7-1.6 2.7-1.4-.3 1.3-1.2 2-2.1 2.2-1 4.2-4.7 6.6-9.8 6.6-4.6 0-7.2-2.6-7.7-7.4Z" fill="#2496ed" />
        <g fill="#2496ed">
          <rect x="4.6" y="9.2" width="2.5" height="2.3" rx="0.4" />
          <rect x="7.7" y="9.2" width="2.5" height="2.3" rx="0.4" />
          <rect x="10.8" y="9.2" width="2.5" height="2.3" rx="0.4" />
          <rect x="13.9" y="9.2" width="2.5" height="2.3" rx="0.4" />
          <rect x="7.7" y="6.3" width="2.5" height="2.3" rx="0.4" />
          <rect x="10.8" y="6.3" width="2.5" height="2.3" rx="0.4" />
          <rect x="10.8" y="3.4" width="2.5" height="2.3" rx="0.4" />
        </g>
      </>
    ),
  },
  java: {
    render: () => (
      <>
        <path d="M9.6 10.8c-1-1.5 1.1-2.5 0-4.2M12.6 10.8c-1-1.5 1.1-2.5 0-4.2" fill="none" stroke="#e76f00" strokeWidth="1.6" strokeLinecap="round" />
        <path d="M5.8 12.8h10.4v2.6a4.6 4.6 0 0 1-4.6 4.6h-1.2a4.6 4.6 0 0 1-4.6-4.6Z" fill="#5382a1" />
        <path d="M16.2 13.8h1.3a2 2 0 0 1 0 4h-1.6" fill="none" stroke="#5382a1" strokeWidth="1.6" />
        <path d="M4.8 21.5h12.4" stroke="#5382a1" strokeWidth="1.6" strokeLinecap="round" />
      </>
    ),
  },
  go: {
    render: () => (
      <>
        <path d="M1.8 10h3.4M1 12.6h3.4M2.6 15.2h2.6" stroke="#00add8" strokeWidth="1.5" strokeLinecap="round" />
        <path d="M12.6 9.4a3.5 3.5 0 1 0 .4 5.2v-2.3h-2.2" fill="none" stroke="#00add8" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        <circle cx="18.6" cy="12" r="3.3" fill="none" stroke="#00add8" strokeWidth="2" />
      </>
    ),
  },
  rust: {
    render: () => (
      <>
        <circle cx="12" cy="12" r="8.4" fill="none" stroke="#e3784b" strokeWidth="2.8" strokeDasharray="1.7 1.6" />
        <circle cx="12" cy="12" r="6.6" fill="none" stroke="#e3784b" strokeWidth="1.6" />
        <path d="M9.4 15.6V8.5h3.3a2 2 0 0 1 0 4H9.4m3.1 0 2.2 3.1" fill="none" stroke="#e3784b" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
      </>
    ),
  },
  electron: {
    render: () => (
      <g fill="none" stroke="#5ab8c9" strokeWidth="1.4">
        <ellipse cx="12" cy="12" rx="9.5" ry="3.6" />
        <ellipse cx="12" cy="12" rx="9.5" ry="3.6" transform="rotate(60 12 12)" />
        <ellipse cx="12" cy="12" rx="9.5" ry="3.6" transform="rotate(120 12 12)" />
        <circle cx="12" cy="12" r="1.6" fill="#5ab8c9" stroke="none" />
      </g>
    ),
  },
  globe: {
    render: () => (
      <g fill="none" stroke="#6b8afd" strokeWidth="1.6" strokeLinecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18M12 3c2.6 2.6 3.8 5.6 3.8 9s-1.2 6.4-3.8 9c-2.6-2.6-3.8-5.6-3.8-9S9.4 5.6 12 3Z" />
      </g>
    ),
  },
  linux: {
    // Tux (Larry Ewing, Simon Budig and Garrett LeSage, via Wikimedia Commons; use permitted
    // with attribution). Blur filters dropped: invisible at icon sizes.
    render: (uid) => (
      <>
        <g transform="translate(2.7 1) scale(0.0859375)">
          <defs id={`${uid}tux-tux_fx`}>
            <linearGradient id={`${uid}tux-gradient_belly_shadow`}>
              <stop offset="0" stopColor="#000000" />
              <stop offset="1" stopColor="#000000" stopOpacity="0.25" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_wing_tip_right_shadow`}>
              <stop offset="0" stopColor="#110800" />
              <stop offset="0.59" stopColor="#a65a00" stopOpacity="0.8" />
              <stop offset="1" stopColor="#ff921e" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_wing_tip_right_glare_1`}>
              <stop offset="0" stopColor="#7c7c7c" />
              <stop offset="1" stopColor="#7c7c7c" stopOpacity="0.33" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_wing_tip_right_glare_2`}>
              <stop offset="0" stopColor="#7c7c7c" />
              <stop offset="1" stopColor="#7c7c7c" stopOpacity="0.33" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_foot_left_layer_1`}>
              <stop offset="0" stopColor="#b98309" />
              <stop offset="1" stopColor="#382605" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_foot_left_glare`}>
              <stop offset="0" stopColor="#ebc40c" />
              <stop offset="1" stopColor="#ebc40c" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_foot_right_shadow`}>
              <stop offset="0" stopColor="#000000" />
              <stop offset="1" stopColor="#000000" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_foot_right_layer_1`}>
              <stop offset="0" stopColor="#3e2a06" />
              <stop offset="1" stopColor="#ad780a" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_foot_right_glare`}>
              <stop offset="0" stopColor="#f3cd0c" />
              <stop offset="1" stopColor="#f3cd0c" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_eyeball`}>
              <stop offset="0" stopColor="#fefefc" />
              <stop offset="0.75" stopColor="#fefefc" />
              <stop offset="1" stopColor="#d4d4d4" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_pupil_left_glare`}>
              <stop offset="0" stopColor="#757574" stopOpacity="0" />
              <stop offset="0.25" stopColor="#757574" />
              <stop offset="0.5" stopColor="#757574" />
              <stop offset="1" stopColor="#757574" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_pupil_right_glare_2`}>
              <stop offset="0" stopColor="#949494" stopOpacity="0.39" />
              <stop offset="0.5" stopColor="#949494" />
              <stop offset="1" stopColor="#949494" stopOpacity="0.39" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_eyelid_left`}>
              <stop offset="0" stopColor="#c8c8c8" />
              <stop offset="1" stopColor="#797978" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_eyelid_right`}>
              <stop offset="0" stopColor="#747474" />
              <stop offset="0.13" stopColor="#8c8c8c" />
              <stop offset="0.25" stopColor="#a4a4a4" />
              <stop offset="0.5" stopColor="#d4d4d4" />
              <stop offset="0.62" stopColor="#d4d4d4" />
              <stop offset="1" stopColor="#7c7c7c" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_eyebrow`}>
              <stop offset="0" stopColor="#646464" stopOpacity="0" />
              <stop offset="0.31" stopColor="#646464" stopOpacity="0.58" />
              <stop offset="0.47" stopColor="#646464" />
              <stop offset="0.73" stopColor="#646464" stopOpacity="0.26" />
              <stop offset="1" stopColor="#646464" stopOpacity="0" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_beak_base`}>
              <stop offset="0" stopColor="#020204" />
              <stop offset="0.73" stopColor="#020204" />
              <stop offset="1" stopColor="#5c5c5c" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_mandible_lower`}>
              <stop offset="0" stopColor="#d2940a" />
              <stop offset="0.75" stopColor="#d89c08" />
              <stop offset="0.87" stopColor="#b67e07" />
              <stop offset="1" stopColor="#946106" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_mandible_upper`}>
              <stop offset="0" stopColor="#ad780a" />
              <stop offset="0.12" stopColor="#d89e08" />
              <stop offset="0.25" stopColor="#edb80b" />
              <stop offset="0.39" stopColor="#ebc80d" />
              <stop offset="0.53" stopColor="#f5d838" />
              <stop offset="0.77" stopColor="#f6d811" />
              <stop offset="1" stopColor="#f5cd31" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_nares`}>
              <stop offset="0" stopColor="#3a2903" />
              <stop offset="0.55" stopColor="#735208" />
              <stop offset="1" stopColor="#ac8c04" />
            </linearGradient>
            <linearGradient id={`${uid}tux-gradient_beak_corner`}>
              <stop offset="0" stopColor="#f5ce2d" />
              <stop offset="1" stopColor="#d79b08" />
            </linearGradient>
            <radialGradient id={`${uid}tux-fill_belly_shadow_left`} href={`#${uid}tux-gradient_belly_shadow`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(61.2,121.2) scale(19,18)" />
            <radialGradient id={`${uid}tux-fill_belly_shadow_right`} href={`#${uid}tux-gradient_belly_shadow`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(125.7,131.6) scale(23.6,18)" />
            <radialGradient id={`${uid}tux-fill_belly_shadow_middle`} href={`#${uid}tux-gradient_belly_shadow`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(94.2,127.5) scale(9.3,10)" />
            <linearGradient id={`${uid}tux-fill_foot_left_base`} href={`#${uid}tux-gradient_foot_left_layer_1`} gradientUnits="userSpaceOnUse" x1="23.2" y1="193" x2="64.3" y2="262" />
            <linearGradient id={`${uid}tux-fill_foot_left_glare`} href={`#${uid}tux-gradient_foot_left_glare`} gradientUnits="userSpaceOnUse" x1="64.5" y1="210.8" x2="77.4" y2="235.2" />
            <linearGradient id={`${uid}tux-fill_foot_right_shadow`} href={`#${uid}tux-gradient_foot_right_shadow`} gradientUnits="userSpaceOnUse" x1="146.9" y1="212" x2="150.2" y2="235.7" />
            <linearGradient id={`${uid}tux-fill_foot_right_base`} href={`#${uid}tux-gradient_foot_right_layer_1`} gradientUnits="userSpaceOnUse" x1="151.5" y1="253" x2="192.9" y2="185.8" />
            <linearGradient id={`${uid}tux-fill_foot_right_glare`} href={`#${uid}tux-gradient_foot_right_glare`} gradientUnits="userSpaceOnUse" x1="162.8" y1="180.7" x2="161.6" y2="191.6" />
            <radialGradient id={`${uid}tux-fill_wing_tip_right_shadow_lower`} href={`#${uid}tux-gradient_wing_tip_right_shadow`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(169.7,194.5) rotate(15) scale(19.7,20.6)" />
            <radialGradient id={`${uid}tux-fill_wing_tip_right_shadow_upper`} href={`#${uid}tux-gradient_wing_tip_right_shadow`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(169.7,189.9) rotate(-2.4) scale(19.7,14.9)" />
            <radialGradient id={`${uid}tux-fill_wing_tip_right_glare_1`} href={`#${uid}tux-gradient_wing_tip_right_glare_1`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(184.7,176.6) rotate(23.5) scale(7,3.2)" />
            <linearGradient id={`${uid}tux-fill_wing_tip_right_glare_2`} href={`#${uid}tux-gradient_wing_tip_right_glare_2`} gradientUnits="userSpaceOnUse" x1="165.7" y1="173.6" x2="168.3" y2="173.5" />
            <radialGradient id={`${uid}tux-fill_eyeball_left`} href={`#${uid}tux-gradient_eyeball`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(86.5,51.4) rotate(-0.6) scale(10.2,15.7)" />
            <linearGradient id={`${uid}tux-fill_pupil_left_glare`} href={`#${uid}tux-gradient_pupil_left_glare`} gradientUnits="userSpaceOnUse" x1="84.3" y1="46.6" x2="89.3" y2="55.6" />
            <radialGradient id={`${uid}tux-fill_eyelid_left`} href={`#${uid}tux-gradient_eyelid_left`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(84.9,43.7) rotate(-9.3) scale(6.2,5.8)" />
            <linearGradient id={`${uid}tux-fill_eyebrow_left`} href={`#${uid}tux-gradient_eyebrow`} gradientUnits="userSpaceOnUse" x1="83.6" y1="32.5" x2="94.5" y2="43.6" />
            <radialGradient id={`${uid}tux-fill_eyeball_right`} href={`#${uid}tux-gradient_eyeball`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(118.1,51.4) rotate(-1.8) scale(13.6,15.7)" />
            <linearGradient id={`${uid}tux-fill_pupil_right_glare`} href={`#${uid}tux-gradient_pupil_right_glare_2`} gradientUnits="userSpaceOnUse" x1="117.9" y1="47.2" x2="123.7" y2="54.1" />
            <linearGradient id={`${uid}tux-fill_eyelid_right`} href={`#${uid}tux-gradient_eyelid_right`} gradientUnits="userSpaceOnUse" x1="112.9" y1="36.2" x2="131.3" y2="47" />
            <linearGradient id={`${uid}tux-fill_eyebrow_right`} href={`#${uid}tux-gradient_eyebrow`} gradientUnits="userSpaceOnUse" x1="119.2" y1="31.6" x2="131.4" y2="43.1" />
            <radialGradient id={`${uid}tux-fill_beak_base`} href={`#${uid}tux-gradient_beak_base`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(97.6,60.1) rotate(-36) scale(11.4,10.4)" />
            <radialGradient id={`${uid}tux-fill_mandible_lower_base`} href={`#${uid}tux-gradient_mandible_lower`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(109.8,70.6) rotate(-22.4) scale(27.1,19.1)" />
            <linearGradient id={`${uid}tux-fill_mandible_upper_base`} href={`#${uid}tux-gradient_mandible_upper`} gradientUnits="userSpaceOnUse" x1="78.1" y1="69.3" x2="126.8" y2="68.9" />
            <radialGradient id={`${uid}tux-fill_naris_left`} href={`#${uid}tux-gradient_nares`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(92.1,59.9) scale(1.3,1.4)" />
            <radialGradient id={`${uid}tux-fill_naris_right`} href={`#${uid}tux-gradient_nares`} gradientUnits="userSpaceOnUse" cx="0" cy="0" r="1" gradientTransform="translate(104.7,59.7) scale(2.8,1.6)" />
            <linearGradient id={`${uid}tux-fill_beak_corner`} href={`#${uid}tux-gradient_beak_corner`} gradientUnits="userSpaceOnUse" x1="126.7" y1="67.5" x2="126.7" y2="71.1" />
            <clipPath id={`${uid}tux-clip_body`}>
              <use href={`#${uid}tux-body_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_wing_left`}>
              <use href={`#${uid}tux-wing_left_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_wing_right`}>
              <use href={`#${uid}tux-wing_right_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_foot_left`}>
              <use href={`#${uid}tux-foot_left_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_foot_right`}>
              <use href={`#${uid}tux-foot_right_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_wing_tip_right`}>
              <use href={`#${uid}tux-wing_tip_right_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_eye_left`}>
              <use href={`#${uid}tux-eyeball_left`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_pupil_left`}>
              <use href={`#${uid}tux-pupil_left_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_eye_right`}>
              <use href={`#${uid}tux-eyeball_right`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_pupil_right`}>
              <use href={`#${uid}tux-pupil_right_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_mandible_lower`}>
              <use href={`#${uid}tux-mandible_lower_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_mandible_upper`}>
              <use href={`#${uid}tux-mandible_upper_base`} />
            </clipPath>
            <clipPath id={`${uid}tux-clip_beak`}>
              <use href={`#${uid}tux-mandible_lower_base`} />
              <use href={`#${uid}tux-mandible_upper_base`} />
            </clipPath>
          </defs>
          <g id={`${uid}tux-tux`}>
            <g id={`${uid}tux-body`}>
              <path id={`${uid}tux-body_base`} fill="#020204" d="m 107,0 c -6,0 -12,1.2 -17.5,4.1 -5.8,3.1 -10.5,8.1 -13.4,14 -2.9,5.9 -4.1,12.2 -4.2,19.1 -0.3,13.1 0.3,26.9 1.3,39.4 0.3,3.8 0.7,6 0.2,9.9 -1.6,8.3 -8.9,13.9 -12.8,21.2 -4.3,8 -6.1,17.1 -9.3,25.6 -3,7.8 -7.1,15.1 -9.9,22.9 -3.9,11 -5.1,23 -2.5,34.4 2,8.7 6.1,16.8 11.6,23.7 -0.8,1.4 -1.6,2.9 -2.4,4.3 -2.6,4.4 -5.7,8.6 -7.2,13.6 -0.7,2.5 -1,5.1 -0.6,7.6 0.5,2.5 1.8,4.9 3.8,6.5 1.3,1 2.9,1.7 4.5,2.1 1.6,0.4 3.3,0.5 5,0.4 6.4,-0.1 12.6,-2.1 18.7,-3.7 3.7,-1 7.3,-1.8 11,-2.6 13.1,-2.7 27.8,-1.6 40,0.1 4.1,0.6 8.2,1.4 12.3,2.4 6.4,1.5 12.7,3.5 19.2,3.7 1.7,0.1 3.5,0 5.1,-0.4 1.7,-0.4 3.3,-1.1 4.7,-2.1 2,-1.6 3.3,-4 3.8,-6.5 0.5,-2.5 0.2,-5.2 -0.6,-7.6 -1.5,-4.9 -4.7,-9.1 -7.3,-13.5 -1,-1.8 -2,-3.5 -3,-5.3 7.9,-8.9 14.3,-19.1 17.9,-30.4 4,-12.3 4.8,-25.6 3.1,-38.4 -1.7,-12.8 -5.8,-25.3 -11.1,-37 -6.7,-14.8 -12.4,-20.1 -16.5,-33.1 -4.4,-14 -0.8,-30.6 -4.1,-43.3 -1.2,-4.3 -3,-8.4 -5.5,-12.2 -2.8,-4.4 -6.4,-8.4 -10.7,-11.5 -6.8,-4.9 -15.3,-7.5 -24,-7.5 z" />
              <path id={`${uid}tux-belly`} fill="#fdfdfb" d="m 83.1,74 c -0.9,1.1 -1.5,2.5 -1.8,3.9 -0.3,1.4 -0.5,2.9 -0.5,4.3 -0.1,2.9 0.1,5.8 -0.7,8.6 -0.8,3 -2.6,5.6 -4.4,8.1 -3.1,4.4 -6.2,8.8 -7.8,14 -0.9,3.1 -1.2,6.4 -0.9,9.6 -3.5,5.1 -6.5,10.5 -9,16.2 -3.8,8.6 -6.4,17.7 -7.3,27 -1.1,11.4 0.3,23.1 4.8,33.7 3.2,7.6 8.1,14.6 14.4,20 3.2,2.8 6.7,5.1 10.5,7 13.1,6.5 29.3,6.5 42.2,-0.4 6.7,-3.6 12.4,-8.8 17.9,-14.2 3.3,-3.2 6.6,-6.5 9.1,-10.3 4.8,-7.4 6.5,-16.4 7.6,-25.2 1.8,-15.4 1.9,-31.6 -4.8,-45.5 -2.3,-4.8 -5.4,-9.2 -9.1,-13.1 -1,-6.7 -2.9,-13.3 -5.8,-19.4 -2,-4.5 -4.5,-8.7 -6.4,-13.2 -0.8,-1.9 -1.5,-3.8 -2.3,-5.6 -0.9,-1.8 -1.9,-3.5 -3.4,-4.9 -1.5,-1.4 -3.3,-2.4 -5.3,-3.1 -1.9,-0.7 -4,-0.9 -6.1,-1 -4.1,-0.2 -8.2,0.3 -12.3,0.2 -3.3,-0.1 -6.5,-0.7 -9.8,-0.5 -1.6,0.1 -3.3,0.4 -4.8,1 -1.5,0.6 -2.9,1.6 -3.9,2.8 z" />
              <g id={`${uid}tux-body_self_shadows`}>
                <path id={`${uid}tux-belly_shadow_left`} opacity="0.25" fill={`url(#${uid}tux-fill_belly_shadow_left)`} clipPath={`url(#${uid}tux-clip_body)`} d="m 68.7,115.2 c 0.9,1.3 -0.6,5.8 19.9,2.9 0,0 -3.6,0.4 -7.1,1.2 -5.5,1.8 -10.3,3.9 -14,6.6 -3.6,2.7 -6.3,6.2 -9.7,9.2 0,0 5.4,-9.9 6.8,-12.9 1.4,-3 -0.2,-2.9 0.8,-7.2 1.1,-4.4 3.7,-8.6 3.7,-8.6 0,0 -2.1,6.2 -0.4,8.8 z" />
                <path id={`${uid}tux-belly_shadow_right`} opacity="0.42" fill={`url(#${uid}tux-fill_belly_shadow_right)`} clipPath={`url(#${uid}tux-clip_body)`} d="m 134.3,114 c -4.2,2.9 -6.6,2.6 -11.6,3.1 -5,0.6 -18.7,0.4 -18.7,0.4 0,0 2,0 6.4,0.8 4.4,0.8 13.3,1.6 18.3,3.5 5,1.9 6.9,2.5 9.9,4.4 4.3,2.8 7.5,7.1 11.7,10.1 0,0 0.2,-4 -1.5,-7 -1.7,-3 -6.2,-7.7 -7.5,-12.1 -1.3,-4.4 -2,-13 -2,-13 0,0 -0.9,7 -5,9.9 z" />
                <path id={`${uid}tux-belly_shadow_middle`} opacity="0.2" fill={`url(#${uid}tux-fill_belly_shadow_middle)`} clipPath={`url(#${uid}tux-clip_body)`} d="m 95.2,107.8 c -0.2,1.2 -0.4,2.5 -0.6,3.7 -0.1,0.6 -0.3,1.2 -0.5,1.8 -0.2,0.6 -0.6,1.1 -1,1.6 -0.4,0.4 -0.9,0.6 -1.4,0.8 -1.9,0.9 -4,1.5 -6.1,1.7 0.8,0.1 1.7,0.1 2.5,0.2 0.5,0.1 1.1,0.1 1.6,0.2 0.5,0.1 1,0.3 1.5,0.7 0.5,0.3 0.8,0.8 1.1,1.3 0.6,1 0.7,2.2 0.8,3.4 0.1,1.5 0.1,3 0,4.5 0.1,-1 0.3,-2 0.6,-2.9 0.6,-1.7 1.5,-3.3 2.7,-4.7 0.5,-0.5 1,-1 1.6,-1.4 1.8,-1.3 4.1,-1.8 6.2,-1.5 -2.2,0.1 -4.4,-0.6 -6.2,-1.9 -0.9,-0.7 -1.7,-1.5 -2.2,-2.5 -0.8,-1.5 -1.1,-3.4 -0.7,-5 z" />
                <path id={`${uid}tux-belly_shadow_lower`} opacity="0.11" fill="#000000" clipPath={`url(#${uid}tux-clip_body)`} d="m 89.8,137.1 c -1.1,4 -1.8,8.2 -2.2,12.3 -0.6,5.9 -0.4,11.8 -0.7,17.7 -0.3,5 -0.8,10 0,15 0.4,2.4 1.1,4.6 2.2,6.8 0.2,-0.8 0.3,-1.6 0.4,-2.5 0.4,-4 -0.3,-8 -0.5,-12 -0.4,-7 0.6,-14 1,-21.1 0.3,-5.4 0.3,-10.8 -0.1,-16.2 z" />
              </g>
              <g id={`${uid}tux-body_glare`}>
                <path id={`${uid}tux-belly_glare`} opacity="0.75" fill="#7c7c7c" clipPath={`url(#${uid}tux-clip_body)`} d="m 160.1,131.2 c 1,-0.2 7.3,5.2 6.5,7.2 -0.9,2 -2.5,0.8 -3.6,0.8 -1.2,0 -4.3,1.5 -4.9,0.6 -0.5,-0.9 1.4,-3 2.4,-4.8 0.8,-1.4 -1.4,-3.6 -0.4,-3.8 z" />
                <path id={`${uid}tux-head_glare`} fill="#7c7c7c" clipPath={`url(#${uid}tux-clip_body)`} d="m 121.5,11.1 c -2.2,1.6 -1.2,3.5 -0.3,5.5 0.9,2 -2.1,7.6 -2.1,7.8 0,0.2 6,-2.9 7.6,-4.9 1.9,-2.4 6.8,3.2 6.6,2.4 0,-1.5 -9.6,-12.3 -11.8,-10.8 z" />
                <path id={`${uid}tux-neck_glare`} fill="#838384" clipPath={`url(#${uid}tux-clip_body)`} d="m 138.3,76.6 c -1.9,1.7 0.9,4.2 2.2,7.2 0.8,1.9 3,4.5 5.2,4.1 1.6,-0.3 2.6,-2.7 2.5,-4.3 -0.3,-3.2 -3,-3.9 -4.9,-5 -1.5,-0.9 -3.6,-3.2 -4.9,-2 z" />
              </g>
            </g>
            <g id={`${uid}tux-wings`}>
              <g id={`${uid}tux-wing_left`}>
                <path id={`${uid}tux-wing_left_base`} fill="#020204" d="m 64,100.9 c -6.1,6.9 -12.4,13.6 -15.8,21.1 -1.7,3.8 -2.5,7.9 -3.7,11.9 -1.3,4.5 -3.1,8.9 -5.1,13.2 -1.9,4 -3.9,7.8 -6,11.7 -1.5,2.8 -3,5.7 -3.5,8.8 -0.4,2.5 -0.2,5 0.5,7.5 0.6,2.4 1.6,4.8 2.8,7 4.9,9.6 12.2,17.8 20.9,24.1 3.9,2.9 8.2,5.3 12.6,7.3 2.4,1.1 4.9,2.1 7.6,2.1 1.3,0 2.6,-0.2 3.9,-0.7 1.2,-0.5 2.3,-1.4 3,-2.5 0.9,-1.4 1.2,-3 1,-4.7 -0.2,-1.6 -0.8,-3.1 -1.6,-4.5 -2.1,-3.4 -5.3,-5.8 -8.4,-8.2 -6.8,-5.2 -13.3,-10.8 -19.6,-16.6 -1.8,-1.6 -3.5,-3.3 -4.8,-5.4 -1.2,-2 -1.9,-4.3 -2.3,-6.6 -1.2,-6.3 -0.9,-13 1.2,-19.1 0.8,-2.4 2,-4.7 3,-6.9 1.9,-4 3.6,-8 6.1,-11.6 3,-4.5 7.1,-8.3 9.6,-13.2 2.1,-4.1 3,-8.7 3.8,-13.2 0.6,-3.5 1.9,-7 2.5,-10.5 -1.2,2.3 -5.1,6.1 -7.5,9.1 z" />
                <path id={`${uid}tux-wing_left_glare`} opacity="0.95" fill="#7c7c7c" clipPath={`url(#${uid}tux-clip_wing_left)`} d="m 57,126.1 c -2,1.8 -3.7,4 -5.1,6.3 -2.3,3.8 -3.6,8.2 -5.3,12.3 -1.2,3.1 -2.7,6.2 -2.9,9.5 -0.1,1.7 0.2,3.4 0.2,5.1 0.1,1.7 -0.1,3.5 -0.9,5 -0.7,1.2 -1.9,2.2 -3.2,2.7 1.8,0.6 3.5,1.8 4.6,3.3 1,1.3 1.6,2.8 2.4,4.2 0.7,1.1 1.5,2.2 2.5,3 1,0.8 2.2,1.4 3.5,1.6 1.8,0.2 3.6,-0.4 5,-1.6 -1.7,-15.6 -0.1,-31.4 4.4,-46.4 0.3,-0.9 0.6,-1.9 0.7,-2.9 0.1,-1 -0.1,-2 -0.7,-2.8 -0.3,-0.4 -0.7,-0.8 -1.2,-1 -0.5,-0.2 -1,-0.3 -1.5,-0.2 -0.5,0.1 -1,0.3 -1.4,0.6 -0.4,0.3 -0.8,0.7 -1,1.2 z" />
              </g>
              <g id={`${uid}tux-wing_right`}>
                <path id={`${uid}tux-wing_right_base`} fill="#020204" d="m 162.8,127.1 c 5.2,4.2 8.6,10.6 9.6,17.2 0.8,5.2 0.3,10.5 -0.9,15.6 -1.2,5.1 -3,10.1 -4.8,15 -0.7,2 -1.4,4 -1.7,6 -0.3,2.1 -0.1,4.3 0.9,6.1 1.1,2.1 3.3,3.6 5.6,4.2 2.3,0.7 4.7,0.7 7,0.1 2.3,-0.6 6.2,-1.3 8,-2.8 4.8,-3.7 5.9,-10.1 7,-15.7 1.2,-5.9 0.6,-12 -0.4,-17.9 -1.4,-8.1 -3.8,-16 -6.8,-23.6 -2.2,-5.6 -5.1,-11 -8.4,-16 -3.3,-4.9 -8,-8.7 -11.5,-13.5 -1.2,-1.7 -2.7,-3.4 -3.8,-5.1 -2.6,-3.6 -2,-2.9 -3.6,-5.3 -1.1,-1.7 -3,-2.3 -4.9,-3 -1.9,-0.7 -4.1,-1 -6,-0.4 -2.6,0.7 -4.7,2.8 -5.9,5.2 -1.1,2.5 -1.3,5.3 -0.9,8 0.6,3.4 2.1,6.6 3.9,9.6 2,3.4 4.3,6.7 7.3,9.2 3.2,2.6 7,4.4 10.2,7 z" />
                <path id={`${uid}tux-wing_right_glare`} fill="#838384" clipPath={`url(#${uid}tux-clip_wing_right)`} d="m 150.4,119 c 0.4,0.4 0.9,0.8 1.3,1.2 3.2,2.6 4.9,5.6 8.2,8.2 5.3,4.2 10.8,11.5 11.8,18.1 0.8,5.2 -0.3,8 -1.6,14.1 -1.3,6.1 -5.1,14.8 -7.1,20.7 -0.8,2.4 1.6,1.4 1.3,3.9 -0.2,1.2 -0.2,2.5 0,3.7 0,-0.2 0,-0.5 0.1,-0.7 0.4,-3.4 1.4,-6.6 2.5,-9.8 2.2,-6.1 4.7,-12.2 6.4,-18.4 1.7,-6.3 1.5,-10.8 0.6,-16.2 -1.1,-6.8 -5.1,-13.1 -10.7,-17.2 -4,-2.9 -8.6,-4.8 -12.9,-7.4 z" />
              </g>
            </g>
            <g id={`${uid}tux-feet`}>
              <g id={`${uid}tux-foot_left`}>
                <path id={`${uid}tux-foot_left_base`} fill={`url(#${uid}tux-fill_foot_left_base)`} d="m 35,175.3 c 1.4,-0.6 2.9,-0.7 4.4,-0.4 1.5,0.3 2.9,0.9 4.1,1.7 2.5,1.7 4.3,4.1 6,6.5 4,5.5 7.9,11.2 11.4,17.1 2.9,4.8 5.5,9.7 8.8,14.2 2.1,2.9 4.6,5.6 6.7,8.6 2.2,2.9 4.1,6.1 5,9.6 1.2,4.5 0.8,9.6 -1.4,13.8 -1.5,3 -3.9,5.5 -6.7,7.2 -2.8,1.7 -5.8,2.5 -9.2,2.5 -5.3,0 -10.4,-2.8 -15.3,-4.8 -10,-4 -20.8,-5.2 -31.1,-8.3 -3.2,-0.9 -6.3,-2.1 -9.4,-3 -1.4,-0.4 -2.9,-0.7 -4.2,-1.3 -1.3,-0.6 -2.6,-1.5 -3.3,-2.8 -0.6,-1 -0.8,-2.1 -0.8,-3.3 0,-1.1 0.3,-2.3 0.7,-3.3 0.8,-2.1 2,-4.1 2.9,-6.2 1.4,-3.4 1.6,-7.2 1.4,-10.9 -0.2,-3.7 -0.8,-7.4 -1,-11.1 -0.1,-1.6 -0.2,-3.3 0.2,-5 0.3,-1.6 1,-3.2 2.2,-4.3 1.1,-1.1 2.5,-1.7 4,-2 1.5,-0.3 3,-0.3 4.5,-0.3 1.5,0 3,0.1 4.6,0 1.5,-0.1 3,-0.5 4.4,-1.2 1.3,-0.7 2.3,-1.8 3.1,-3 0.8,-1.2 1.5,-2.5 2.2,-3.7 0.7,-1.3 1.4,-2.5 2.4,-3.6 0.9,-1.1 2.1,-2.1 3.4,-2.6 z" />
                <path id={`${uid}tux-foot_left_layer_1`} fill="#d99a03" clipPath={`url(#${uid}tux-clip_foot_left)`} d="m 37.2,177.7 c 1.2,-0.5 2.7,-0.6 4,-0.3 1.3,0.3 2.5,0.9 3.6,1.8 2.1,1.6 3.6,4 5,6.3 3.4,5.5 6.8,11.2 9.9,16.9 2.4,4.3 4.7,8.7 7.6,12.7 1.9,2.6 4.2,5 6.2,7.6 2,2.6 3.8,5.4 4.6,8.6 1.1,4 0.7,8.5 -1.3,12.3 -1.4,2.7 -3.6,5 -6.2,6.5 -2.6,1.5 -5.7,2.3 -8.7,2.1 -4.8,-0.2 -9.2,-2.6 -13.8,-4.3 -8.7,-3.2 -18.1,-3.6 -27.1,-6 -3.2,-0.9 -6.3,-2 -9.5,-2.8 -1.4,-0.4 -2.9,-0.7 -4.2,-1.2 -1.4,-0.6 -2.6,-1.4 -3.4,-2.7 -0.5,-0.9 -0.8,-2.1 -0.7,-3.1 0,-1.1 0.3,-2.2 0.7,-3.2 0.8,-2 2,-3.9 2.8,-5.9 1.2,-3.1 1.3,-6.5 1.1,-9.8 -0.2,-3.3 -0.8,-6.6 -1,-9.8 -0.1,-1.5 -0.1,-3 0.2,-4.4 0.3,-1.4 0.9,-2.9 2,-3.9 1.1,-1.1 2.7,-1.7 4.3,-2 1.6,-0.2 3.2,-0.1 4.8,0 1.6,0.1 3.2,0.3 4.8,0.2 1.6,-0.1 3.2,-0.5 4.5,-1.4 1.2,-0.8 2.1,-2 2.8,-3.3 0.7,-1.3 1.2,-2.6 1.7,-4 0.5,-1.4 1.1,-2.7 1.9,-3.9 0.8,-1.2 1.9,-2.2 3.3,-2.8 z" />
                <path id={`${uid}tux-foot_left_layer_2`} fill="#f5bd0c" clipPath={`url(#${uid}tux-clip_foot_left)`} d="m 36,174.6 c 1.2,-0.6 2.6,-0.7 4,-0.5 1.3,0.3 2.6,0.9 3.6,1.8 2.1,1.7 3.4,4.1 4.7,6.5 2.8,5.5 5.5,11 8.9,16.2 2.5,3.7 5.3,7.2 7.8,10.9 3.4,5 6.2,10.6 7.3,16.5 0.8,4 0.7,8.3 -1.2,11.9 -1.3,2.4 -3.3,4.3 -5.8,5.5 -2.4,1.2 -5.2,1.7 -7.9,1.5 -4.3,-0.4 -8.1,-2.5 -12.2,-3.9 -7.1,-2.6 -14.8,-3.2 -22.2,-5.2 -3.1,-0.8 -6.1,-1.9 -9.3,-2.5 -1.4,-0.3 -2.8,-0.5 -4.1,-1 -1.3,-0.5 -2.6,-1.3 -3.2,-2.5 -0.5,-0.9 -0.6,-1.9 -0.6,-2.8 0.1,-1 0.4,-1.9 0.7,-2.8 0.8,-1.8 1.9,-3.5 2.5,-5.3 0.9,-2.8 0.5,-5.7 -0.1,-8.5 -0.7,-2.8 -1.6,-5.5 -1.9,-8.4 -0.1,-1.3 -0.1,-2.6 0.2,-3.8 0.3,-1.2 0.9,-2.5 1.8,-3.3 1.2,-1.1 2.9,-1.7 4.6,-1.9 1.7,-0.2 3.4,0.1 5,0.3 1.7,0.2 3.3,0.5 5,0.4 1.7,-0.1 3.4,-0.5 4.7,-1.5 1.3,-1 2.1,-2.5 2.6,-4.1 0.5,-1.6 0.6,-3.2 0.8,-4.8 0.2,-1.6 0.4,-3.3 1.1,-4.8 0.6,-1.5 1.7,-2.9 3.2,-3.6 z" />
                <path id={`${uid}tux-foot_left_glare`} fill={`url(#${uid}tux-fill_foot_left_glare)`} clipPath={`url(#${uid}tux-clip_foot_left)`} d="m 51.2,188.2 c 2.2,4.1 3.6,8.7 5.8,12.8 2,3.8 4.4,7.7 6.5,11.1 0.9,1.6 3.1,3.9 5.3,7.6 2,3.3 4,8 5.1,9.6 -0.6,-1.8 -2,-6.8 -3.5,-10.3 -1.5,-3.3 -3.2,-5.2 -4.2,-6.9 -2.1,-3.5 -4.3,-6.6 -6.5,-9.9 -3,-4.6 -5.2,-9.7 -8.4,-14 z" />
              </g>
              <g id={`${uid}tux-foot_right`}>
                <path id={`${uid}tux-foot_right_shadow`} opacity="0.2" fill={`url(#${uid}tux-fill_foot_right_shadow)`} clipPath={`url(#${uid}tux-clip_body)`} d="m 198.7,215.6 c -0.4,1.3 -1,2.6 -1.8,3.8 -1.8,2.6 -4.3,4.5 -6.8,6.3 -4.3,3.1 -8.8,5.9 -12.9,9.4 -2.7,2.3 -5.2,5 -7.5,7.7 -1.9,2.4 -3.8,4.8 -6,6.9 -2.2,2.1 -4.9,3.8 -7.7,4.8 -3.5,1.1 -7.1,1.1 -10.5,0.2 -2.3,-0.6 -4.6,-1.6 -6.1,-3.5 -1.4,-1.9 -1.9,-4.4 -2.1,-6.9 -0.3,-4.4 0.2,-8.9 0.7,-13.4 0.4,-3.7 0.8,-7.5 0.9,-11.2 0.3,-6.8 -0.2,-13.5 -1.1,-20.1 -0.2,-1.1 -0.3,-2.2 -0.2,-3.4 0.1,-1.1 0.5,-2.3 1.3,-3.2 0.7,-0.8 1.8,-1.3 2.8,-1.5 1,-0.2 2.1,-0.2 3.1,-0.3 2.4,-0.1 4.9,-0.2 7.2,0 1.5,0.1 3,0.4 4.5,0.6 2.5,0.2 5.1,0.1 7.6,-0.6 2.7,-0.8 5.3,-2.3 8.1,-2.7 1.1,-0.2 2.3,-0.1 3.4,0 1.1,0.1 2.2,0.5 3.1,1.1 0.7,0.5 1.1,1.2 1.5,2 0.6,1.1 0.9,2.4 1.1,3.7 0.2,1.1 0.2,2.3 0.5,3.4 0.5,1.8 1.6,3.4 2.9,4.6 1.3,1.2 2.9,2.2 4.4,3.2 1.5,1 3.1,1.9 4.7,2.7 0.8,0.4 1.6,0.7 2.3,1.1 0.7,0.4 1.4,1 1.9,1.7 0.7,0.9 0.9,2.2 0.7,3.4 z" />
                <path id={`${uid}tux-foot_right_base`} fill={`url(#${uid}tux-fill_foot_right_base)`} d="m 213.5,222.9 c -2.3,2.7 -5.4,4.5 -8.5,6 -5.3,2.7 -10.9,5.1 -15.9,8.4 -3.4,2.2 -6.5,4.8 -9.4,7.5 -2.5,2.4 -4.8,4.9 -7.6,6.9 -2.8,2 -6,3.5 -9.5,4 -0.9,0.1 -1.9,0.2 -2.9,0.2 -3.2,0 -6.5,-0.8 -9.5,-2.1 -2.7,-1.2 -5.3,-2.9 -6.9,-5.4 -1.6,-2.5 -1.9,-5.6 -1.9,-8.6 0,-5.2 1,-10.4 1.9,-15.6 0.8,-4.3 1.5,-8.6 1.9,-12.9 0.8,-7.9 0.8,-15.8 0.3,-23.7 -0.1,-1.3 -0.2,-2.6 0,-4 0.2,-1.3 0.7,-2.6 1.7,-3.5 0.9,-0.8 2.2,-1.2 3.4,-1.2 1.2,-0.1 2.4,0.1 3.6,0.3 2.9,0.4 5.7,0.7 8.5,1.5 1.8,0.5 3.5,1.1 5.2,1.6 2.9,0.8 6,1.2 9,0.8 3.2,-0.4 6.4,-1.6 9.7,-1.5 1.3,0 2.7,0.3 4,0.7 1.3,0.4 2.6,1 3.5,2 0.7,0.7 1.2,1.7 1.6,2.6 0.6,1.4 0.9,3 1,4.5 0.1,1.4 0.1,2.8 0.3,4.1 0.4,2.2 1.6,4.2 3,6 1.4,1.7 3.2,3.2 4.9,4.7 1.7,1.4 3.5,2.9 5.3,4.2 0.9,0.6 1.8,1.2 2.6,1.8 0.8,0.7 1.6,1.4 2.1,2.3 0.5,0.8 0.7,1.7 0.7,2.6 0,0.5 -0.1,1 -0.2,1.6 -0.5,1.4 -1.3,2.8 -2.3,4 z" />
                <path id={`${uid}tux-foot_right_layer_1`} fill="#cd8907" clipPath={`url(#${uid}tux-clip_foot_right)`} d="m 213.2,216.1 c -0.5,1.3 -1.3,2.6 -2.2,3.7 -2.1,2.4 -4.9,4 -7.8,5.4 -4.9,2.4 -9.9,4.6 -14.5,7.5 -3.1,2 -5.9,4.3 -8.6,6.8 -2.3,2.1 -4.4,4.4 -7,6.2 -2.6,1.8 -5.5,3.1 -8.7,3.6 -3.8,0.6 -7.7,-0.2 -11.2,-1.7 -2.5,-1.1 -4.8,-2.6 -6.3,-4.8 -1.4,-2.3 -1.8,-5 -1.8,-7.7 0,-4.7 0.9,-9.4 1.7,-14 0.7,-3.9 1.4,-7.7 1.8,-11.6 0.8,-7.1 0.7,-14.2 0.2,-21.3 -0.1,-1.2 -0.2,-2.4 0,-3.6 0.2,-1.2 0.7,-2.4 1.6,-3.1 0.8,-0.7 2,-1.1 3.1,-1.1 1.1,-0.1 2.2,0.1 3.3,0.3 2.6,0.4 5.2,0.7 7.8,1.3 1.6,0.4 3.2,1 4.8,1.4 2.7,0.7 5.5,1.1 8.2,0.7 3,-0.3 5.8,-1.4 8.8,-1.4 1.2,0 2.4,0.3 3.6,0.6 1.2,0.4 2.4,0.9 3.2,1.8 0.7,0.7 1.1,1.5 1.5,2.4 0.5,1.3 0.9,2.7 0.9,4.1 0,1.5 -0.3,2.9 -0.1,4.4 0.2,1.2 0.7,2.3 1.3,3.3 0.6,1 1.4,1.9 2.2,2.8 1.5,1.8 3,3.5 4.8,4.9 2.1,1.5 4.6,2.4 6.8,3.9 0.7,0.4 1.3,0.9 1.7,1.5 0.8,1.1 1,2.5 0.5,3.7 z" />
                <path id={`${uid}tux-foot_right_layer_2`} fill="#f5c021" clipPath={`url(#${uid}tux-clip_foot_right)`} d="m 212.9,214.6 c -0.6,1.4 -1.4,2.6 -2.3,3.7 -2.1,2.6 -5,4.3 -8,5.5 -5,1.9 -10.4,2.1 -15.3,4.3 -3,1.3 -5.7,3.3 -8.4,5.1 -2.2,1.5 -4.4,2.9 -6.8,4 -2.7,1.2 -5.6,1.9 -8.5,2.3 -1.8,0.3 -3.7,0.5 -5.6,0.4 -1.9,-0.1 -3.7,-0.5 -5.4,-1.5 -1.2,-0.7 -2.4,-1.8 -3,-3.1 -0.7,-1.5 -0.9,-3.2 -0.8,-4.9 0.1,-4.5 1,-9 1,-13.5 0,-3.9 -0.8,-7.8 -1,-11.7 -0.4,-7.5 1.2,-15.2 -0.2,-22.6 -0.2,-1.2 -0.5,-2.4 -0.5,-3.6 0,-0.6 0.1,-1.2 0.3,-1.8 0.2,-0.6 0.6,-1.1 1,-1.4 0.4,-0.3 0.8,-0.5 1.3,-0.6 0.5,-0.1 0.9,-0.1 1.4,0 0.9,0.1 1.8,0.5 2.7,0.8 2.6,0.8 5.3,0.8 8,1.4 1.7,0.4 3.3,1 4.9,1.4 2.8,0.8 5.6,1.1 8.5,0.8 3,-0.4 6,-1.5 9.1,-1.4 1.3,0 2.5,0.3 3.7,0.6 1.2,0.4 2.4,0.9 3.3,1.8 0.7,0.7 1.1,1.5 1.5,2.4 0.6,1.3 1,2.7 0.9,4.1 0,0.7 -0.1,1.5 -0.2,2.2 -0.1,0.7 -0.1,1.5 0.1,2.2 0.2,0.7 0.6,1.4 1,2 0.4,0.6 0.9,1.1 1.3,1.7 1.2,1.6 2.1,3.5 3,5.3 0.9,1.8 2,3.6 3.5,4.9 2,1.8 4.7,2.5 7,3.9 0.7,0.4 1.3,0.9 1.8,1.6 0.4,0.5 0.6,1.1 0.7,1.8 0.1,0.7 0,1.3 -0.2,2 z" />
                <path id={`${uid}tux-foot_right_glare`} fill={`url(#${uid}tux-fill_foot_right_glare)`} clipPath={`url(#${uid}tux-clip_foot_right)`} d="m 148.1,181.6 c 2.8,-0.8 5.2,1.4 7.3,3 1.3,1.1 3.2,0.8 4.9,0.9 2.7,-0.1 5.4,0.5 8.1,0.1 5.3,-0.5 10.5,-1.8 15.8,-2 2.5,-0.2 5.4,-0.3 7.6,1.2 0.9,0.6 2.2,3.2 3.1,2.2 -0.4,-2.7 -2.4,-5.4 -5.1,-6.1 -2.1,-0.3 -4.3,0.2 -6.5,0.1 -6.4,-0.1 -12.8,-1.3 -19.2,-1 -4.5,0 -8.9,-0.2 -13.4,-0.3 -1.8,-0.4 -2.4,1.2 -3.3,1.8 0.2,0.2 0.3,0.2 0.8,0.1 z" />
              </g>
            </g>
            <g id={`${uid}tux-wing_tip_right`}>
              <g id={`${uid}tux-wing_tip_right_shadow`}>
                <path id={`${uid}tux-wing_tip_right_shadow_lower`} opacity="0.35" fill={`url(#${uid}tux-fill_wing_tip_right_shadow_lower)`} clipPath={`url(#${uid}tux-clip_foot_right)`} d="m 185.5,187.6 c -0.5,-0.9 -1.4,-1.7 -2.4,-2.1 -1,-0.4 -2.1,-0.6 -3.1,-0.5 -2.1,0 -4.2,0.6 -6.4,0.4 -1.8,-0.2 -3.5,-0.8 -5.2,-1.3 -1.8,-0.4 -3.7,-0.6 -5.5,-0.1 -1.9,0.5 -3.6,1.8 -4.7,3.5 -0.9,1.5 -1.4,3.2 -1.5,5 -0.1,1.8 0,3.5 0.2,5.2 0.1,1.3 0.3,2.5 0.6,3.8 0.3,1.2 0.8,2.4 1.5,3.5 1.1,1.5 2.7,2.5 4.4,3.2 2.8,1 6,1 8.7,-0.1 4.9,-2.1 9,-5.9 11.5,-10.6 0.9,-1.7 1.6,-3.5 2.1,-5.3 0.2,-0.7 0.3,-1.5 0.3,-2.2 0,-0.8 -0.1,-1.5 -0.5,-2.2 z" />
                <path id={`${uid}tux-wing_tip_right_shadow_upper`} opacity="0.35" fill={`url(#${uid}tux-fill_wing_tip_right_shadow_upper)`} clipPath={`url(#${uid}tux-clip_foot_right)`} d="m 185.5,184.9 c -0.5,-0.7 -1.4,-1.2 -2.4,-1.5 -1,-0.3 -2.1,-0.4 -3.1,-0.4 -2.1,0 -4.2,0.4 -6.4,0.3 -1.8,-0.1 -3.5,-0.6 -5.2,-0.9 -1.8,-0.3 -3.7,-0.5 -5.5,-0.1 -1.9,0.4 -3.6,1.3 -4.7,2.5 -0.9,1.1 -1.4,2.3 -1.5,3.6 -0.1,1.3 0,2.5 0.2,3.8 0.1,0.9 0.3,1.8 0.6,2.7 0.3,0.9 0.8,1.8 1.5,2.5 1.1,1.1 2.7,1.8 4.4,2.3 2.8,0.8 6,0.7 8.7,-0.1 4.9,-1.5 9,-4.3 11.5,-7.7 0.9,-1.2 1.6,-2.5 2.1,-3.8 0.2,-0.5 0.3,-1.1 0.3,-1.6 0,-0.6 -0.1,-1.1 -0.5,-1.6 z" />
              </g>
              <path id={`${uid}tux-wing_tip_right_base`} fill="#020204" d="m 189.6,178.7 c -0.3,-0.9 -1,-1.8 -1.7,-2.5 -0.8,-0.7 -1.6,-1.2 -2.6,-1.6 -1.9,-0.8 -3.9,-1.1 -5.9,-1.5 -1.9,-0.3 -3.7,-0.8 -5.6,-1.1 -2,-0.3 -4,-0.6 -5.9,-0.1 -1.7,0.4 -3.3,1.3 -4.5,2.4 -1.3,1.2 -2.3,2.6 -3,4.2 -1.3,2.8 -1.7,5.9 -1.4,9 0.2,2.3 0.8,4.6 2.2,6.3 1.2,1.4 2.9,2.3 4.7,2.8 3,0.9 6.3,0.8 9.4,-0.1 5,-1.7 9.4,-5 12.3,-9.3 1,-1.4 1.8,-3 2.2,-4.7 0.3,-1.3 0.3,-2.7 -0.1,-3.9 z" />
              <g id={`${uid}tux-wing_tip_right_glare`}>
                <defs>
                  <path id={`${uid}tux-path_wing_tip_right_glare`} d="m 168.9,171.1 c -0.5,0 -0.9,0.1 -1.4,0.2 -3,0.5 -5.7,2.4 -7.3,5 -1.1,1.9 -1.6,4 -1.4,6.2 0.1,-1.5 0.5,-3 1.2,-4.3 1.2,-2.2 3.5,-3.9 6,-4.4 1.5,-0.3 3.1,-0.1 4.7,0.1 1.5,0.2 3,0.4 4.5,0.7 2.3,0.5 4.5,1.3 6.4,2.6 0.4,0.3 0.9,0.7 1.2,1.1 0.3,0.4 0.6,0.9 0.7,1.4 0.1,0.8 -0.1,1.6 -0.5,2.3 -0.4,0.7 -0.9,1.3 -1.5,1.9 -0.5,0.5 -0.9,0.9 -1.4,1.3 2.2,-0.4 4.4,-1 6.3,-2.3 0.8,-0.6 1.5,-1.2 1.9,-2 0.5,-0.8 0.7,-1.8 0.4,-2.8 -0.2,-0.6 -0.5,-1.2 -0.9,-1.7 -0.4,-0.5 -0.9,-0.9 -1.4,-1.2 -2.3,-1.6 -5,-2.6 -7.8,-3 -1.8,-0.3 -3.6,-0.6 -5.4,-0.8 -1.4,-0.2 -2.9,-0.3 -4.3,-0.2 z" />
                </defs>
                <use id={`${uid}tux-wing_tip_right_glare_1`} href={`#${uid}tux-path_wing_tip_right_glare`} fill={`url(#${uid}tux-fill_wing_tip_right_glare_1)`} clipPath={`url(#${uid}tux-clip_wing_tip_right)`} />
                <use id={`${uid}tux-wing_tip_right_glare_2`} href={`#${uid}tux-path_wing_tip_right_glare`} fill={`url(#${uid}tux-fill_wing_tip_right_glare_2)`} clipPath={`url(#${uid}tux-clip_wing_tip_right)`} />
              </g>
            </g>
            <g id={`${uid}tux-face`}>
              <g id={`${uid}tux-eyes`}>
                <g id={`${uid}tux-eye_left`}>
                  <path id={`${uid}tux-eyeball_left`} fill={`url(#${uid}tux-fill_eyeball_left)`} d="m 84.5,38.3 c -1.5,0.1 -3,0.8 -4.1,1.8 -1.1,1.1 -1.9,2.4 -2.4,3.9 -1,2.9 -0.8,6.1 -0.5,9.2 0.2,2.8 0.4,5.6 1.4,8.2 0.5,1.3 1.2,2.5 2.2,3.5 1,1 2.2,1.8 3.5,2.1 1.3,0.3 2.6,0.3 3.9,-0.1 1.2,-0.4 2.4,-1.1 3.3,-2 1.4,-1.3 2.3,-3.1 2.8,-4.9 0.5,-1.8 0.7,-3.8 0.7,-5.7 0,-2.4 -0.3,-4.8 -1.1,-7 -0.8,-2.3 -1.9,-4.4 -3.6,-6.1 -0.8,-0.8 -1.7,-1.6 -2.8,-2.1 -1,-0.5 -2.2,-0.8 -3.4,-0.7 z" />
                  <g id={`${uid}tux-pupil_left`}>
                    <path id={`${uid}tux-pupil_left_base`} fill="#020204" d="m 80.8,51 c -0.3,1.9 -0.3,4 0.3,5.8 0.4,1.2 1.2,2.3 2,3.3 0.6,0.6 1.2,1.2 2,1.6 0.8,0.4 1.6,0.5 2.5,0.3 0.8,-0.2 1.4,-0.7 1.9,-1.3 0.5,-0.6 0.8,-1.3 1.1,-2.1 0.7,-2.2 0.6,-4.6 -0.1,-6.8 -0.5,-1.6 -1.3,-3.2 -2.6,-4.2 -0.6,-0.5 -1.3,-0.9 -2.1,-1.1 -0.8,-0.2 -1.6,-0.1 -2.4,0.2 -0.8,0.4 -1.4,1.1 -1.9,1.9 -0.4,0.8 -0.6,1.7 -0.8,2.5 z" />
                    <path id={`${uid}tux-pupil_left_glare`} fill={`url(#${uid}tux-fill_pupil_left_glare)`} clipPath={`url(#${uid}tux-clip_pupil_left)`} d="m 84.8,49.6 c 0.2,0.6 0.9,0.8 1.3,1.2 0.4,0.4 0.8,0.9 1,1.4 0.4,1 -0.4,2.5 0.4,3.2 0.2,0.2 0.8,0.2 1,0 1,-0.9 0.8,-2.7 0.4,-4 -0.4,-1.1 -1.2,-2.2 -2.3,-2.8 -0.5,-0.3 -1.2,-0.5 -1.7,-0.1 -0.3,0.2 -0.2,0.7 -0.1,1.1 z" />
                  </g>
                  <path id={`${uid}tux-eyelid_left`} fill={`url(#${uid}tux-fill_eyelid_left)`} clipPath={`url(#${uid}tux-clip_eye_left)`} d="m 81.1,44.5 c 2.3,-1.4 5.1,-1.7 7.8,-1.4 2.7,0.3 5.3,1 7.9,1.8 1.9,0.5 3.8,1 5.6,1.8 1.8,0.8 3.4,2.1 4.4,3.8 0.2,0.3 0.3,0.6 0.5,0.8 0.2,0.3 0.4,0.5 0.6,0.7 0.2,0.2 0.6,0.3 0.9,0.3 0.2,0 0.3,-0.1 0.5,-0.1 0.1,-0.1 0.3,-0.2 0.3,-0.3 0.1,-0.2 0.1,-0.3 0.1,-0.5 0,-0.2 -0.1,-0.4 -0.1,-0.5 -0.7,-2.4 -2.2,-4.4 -3.4,-6.5 -0.7,-1.3 -1.3,-2.7 -2,-4 -2.4,-4.5 -6.4,-7.9 -10.9,-10.2 -4.5,-2.3 -9.6,-3.4 -14.7,-3.6 -5.9,-0.3 -11.7,0.3 -17.5,1.4 -2.5,0.4 -5.1,1 -7.3,2.2 -1.1,0.6 -2.1,1.4 -2.9,2.4 -0.8,1 -1.4,2.2 -1.6,3.4 -0.2,1.2 0,2.5 0.4,3.6 0.4,1.1 1.1,2.2 1.9,3.1 1.5,1.9 3.5,3.4 5.4,5 1.8,1.6 3.5,3.4 5.6,4.8 1,0.7 2.1,1.2 3.3,1.6 1.2,0.3 2.4,0.4 3.6,0.2 1.2,-0.2 2.4,-0.9 3.4,-1.6 1,-0.8 1.9,-1.7 2.7,-2.7 1.7,-1.9 3.2,-4 5.4,-5.3 z" />
                  <path id={`${uid}tux-eyebrow_left`} fill={`url(#${uid}tux-fill_eyebrow_left)`} d="m 90.8,36.6 c 2.2,2 3.8,4.5 4.8,7.2 -0.5,-2.9 -1.2,-5.3 -3.1,-7.2 -1.2,-1.1 -2.5,-2 -4,-2.7 -1.3,-0.6 -2.8,-1 -3.6,-1 -0.8,0 -1,0 -1.2,0 -0.2,0 -0.3,0 0.2,0.1 0.5,0.1 1.8,0.4 3,1 1.3,0.6 2.6,1.5 3.8,2.6 z" />
                </g>
                <g id={`${uid}tux-eye_right`}>
                  <path id={`${uid}tux-eyeball_right`} fill={`url(#${uid}tux-fill_eyeball_right)`} d="m 111.6,38.3 c -2.4,1.6 -4.4,3.9 -5.4,6.7 -1.2,3.5 -0.8,7.3 0.4,10.8 1.2,3.5 3.3,6.9 6.4,9.1 1.5,1.1 3.3,1.8 5.2,2 1.9,0.2 3.8,-0.1 5.5,-0.9 2,-1.1 3.6,-2.9 4.5,-5 1,-2.1 1.4,-4.4 1.5,-6.7 0.2,-2.9 -0.1,-5.9 -1.1,-8.6 -1,-3 -2.9,-5.8 -5.6,-7.5 -1.3,-0.9 -2.8,-1.4 -4.4,-1.7 -1.6,-0.2 -3.2,-0.1 -4.7,0.5 -0.8,0.3 -1.6,0.8 -2.4,1.3 z" />
                  <g id={`${uid}tux-pupil_right`}>
                    <path id={`${uid}tux-pupil_right_base`} fill="#020204" d="m 117.1,45.5 c -0.9,0.1 -1.8,0.4 -2.5,0.8 -0.8,0.5 -1.4,1.1 -1.9,1.9 -1,1.5 -1.5,3.3 -1.6,5.1 -0.1,1.4 0.1,2.7 0.6,4 0.5,1.3 1.2,2.5 2.2,3.3 1.1,0.9 2.4,1.4 3.8,1.5 1.4,0.1 2.8,-0.3 3.9,-1.1 0.9,-0.6 1.6,-1.5 2.2,-2.5 0.5,-1 0.8,-2 1,-3.1 0.2,-1.9 0,-4 -0.9,-5.7 -0.9,-1.7 -2.4,-3.2 -4.2,-3.8 -0.8,-0.3 -1.6,-0.4 -2.5,-0.4 z" />
                    <path id={`${uid}tux-pupil_right_glare`} fill={`url(#${uid}tux-fill_pupil_right_glare)`} clipPath={`url(#${uid}tux-clip_pupil_right)`} d="m 122.7,53.4 c 1,-1 -0.7,-3.6 -2,-4.7 -1,-0.8 -3.8,-1.6 -3.7,-0.8 0.1,0.8 1.4,1.9 2.2,2.8 1.1,1 3,3.2 3.5,2.7 z" />
                  </g>
                  <path id={`${uid}tux-eyelid_right`} fill={`url(#${uid}tux-fill_eyelid_right)`} clipPath={`url(#${uid}tux-clip_eye_right)`} d="m 102.6,47 c 2.1,-1.7 4.5,-3 7,-3.8 5.2,-1.6 11.2,-1 15.8,2 1.6,1 3,2.3 4.5,3.5 1.5,1.2 3.1,2.3 4.8,2.9 1,0.3 2,0.5 3,0.4 0.9,-0.1 1.8,-0.4 2.6,-1 0.8,-0.5 1.4,-1.2 1.9,-2 1,-1.6 1.3,-3.5 1.1,-5.3 -0.3,-3.7 -2.4,-6.9 -4.2,-10.2 -0.6,-1 -1.1,-2.1 -1.7,-3 -2,-2.9 -5.2,-4.9 -8.6,-6 -3.4,-1.1 -6.9,-1.5 -10.5,-2 -1.6,-0.2 -3.2,-0.4 -4.7,-0.2 -1.8,0.2 -3.5,0.9 -5.3,1.4 -0.8,0.2 -1.7,0.4 -2.5,0.6 -0.8,0.2 -1.6,0.6 -2.3,1.1 -1,0.8 -1.6,2 -1.9,3.2 -0.3,1.2 -0.2,2.5 0,3.8 0.4,2.5 1.3,4.9 1.2,7.5 -0.1,1.7 -0.6,3.4 -0.5,5.2 0,0.5 0.1,1.6 0.2,2 z" />
                  <path id={`${uid}tux-eyebrow_right`} fill={`url(#${uid}tux-fill_eyebrow_right)`} d="m 119.9,31.2 c -0.4,0.5 -0.8,1.1 -1.1,1.7 1.9,0.4 3.6,1.2 5.2,2.2 3.1,2 5.4,5 7,8.3 0.4,-0.4 0.7,-0.9 1,-1.4 -1.6,-3.4 -4,-6.5 -7.2,-8.7 -1.5,-1 -3.2,-1.8 -5,-2.2 z" />
                </g>
              </g>
              <g id={`${uid}tux-beak`}>
                <g id={`${uid}tux-beak_shadow`}>
                  <path id={`${uid}tux-beak_shadow_lower`} fill="#000000" fillOpacity="0.258824" clipPath={`url(#${uid}tux-clip_body)`} d="m 81.1,89.3 c 1.5,4.3 4.4,7.9 7.9,10.7 1.2,0.9 2.4,1.8 3.8,2.4 1.4,0.6 2.9,1 4.4,0.8 1.5,-0.1 2.9,-0.7 4.1,-1.4 1.3,-0.7 2.5,-1.6 3.7,-2.4 2.1,-1.4 4.4,-2.4 6.6,-3.6 2.6,-1.4 5.2,-3.1 7.5,-5.1 1,-0.9 2,-1.9 3.1,-2.8 1.1,-0.8 2.5,-1.4 3.9,-1.3 1,0.1 2,0.5 3,0.7 0.5,0.1 1,0.2 1.6,0.1 0.5,-0.1 1,-0.3 1.4,-0.7 0.4,-0.5 0.6,-1.1 0.6,-1.8 0,-0.6 -0.2,-1.2 -0.5,-1.8 -0.6,-1.1 -1.5,-2.1 -1.9,-3.3 -0.4,-1.1 -0.3,-2.2 -0.3,-3.4 0,-1.1 0,-2.3 -0.5,-3.3 -0.4,-0.8 -1.1,-1.4 -1.8,-1.8 -0.8,-0.4 -1.6,-0.6 -2.5,-0.7 -1.7,-0.2 -3.4,0.2 -5.2,0.3 -2.3,0.1 -4.6,-0.1 -6.9,0 -2.9,0.2 -5.7,1 -8.5,1 -3.3,0 -6.5,-1.1 -9.7,-0.6 -1.4,0.2 -2.7,0.7 -4,1.2 -1.3,0.5 -2.7,0.8 -4.1,0.8 -1.6,0 -3.1,-0.6 -4.7,-0.7 -0.8,-0.1 -1.6,0 -2.4,0.3 -0.7,0.3 -1.4,0.8 -1.8,1.5 -0.2,0.4 -0.3,0.9 -0.3,1.3 0,0.5 0,0.9 0.1,1.4 0.2,0.9 0.6,1.7 1,2.5 1.2,3.1 1.5,6.4 2.5,9.5 z" />
                  <path id={`${uid}tux-beak_shadow_upper`} opacity="0.3" fill="#000000" clipPath={`url(#${uid}tux-clip_body)`} d="m 77,77.2 c 2.9,1.8 5.4,3.9 7.6,6.4 2,2.3 3.7,4.9 6.3,6.6 1.8,1.2 4,1.9 6.3,2.1 2.6,0.2 5.3,-0.1 7.8,-0.8 2.4,-0.6 4.6,-1.5 6.7,-2.7 4,-2.2 7.3,-5.5 11.7,-7 0.9,-0.3 1.9,-0.6 2.9,-1 0.9,-0.4 1.8,-1 2.2,-1.8 0.4,-0.8 0.4,-1.8 0.5,-2.6 0.1,-1 0.5,-1.9 0.7,-2.8 0.2,-0.9 0.2,-2 -0.2,-2.8 -0.4,-0.7 -1,-1.3 -1.8,-1.7 -0.8,-0.4 -1.6,-0.5 -2.5,-0.6 -1.7,-0.1 -3.4,0.3 -5.2,0.5 -2.3,0.2 -4.6,-0.1 -6.9,0 -2.9,0.1 -5.7,0.7 -8.5,0.8 -3.2,0.1 -6.5,-0.3 -9.7,-0.4 -1.4,-0.1 -2.8,0 -4.2,0.2 -1.4,0.2 -2.8,0.7 -3.9,1.4 -1.1,0.7 -2.1,1.6 -3.1,2.5 -0.5,0.4 -1.1,0.8 -1.7,1.1 -0.6,0.3 -1.3,0.5 -2,0.5 -0.3,0 -0.7,0 -1.1,0 -0.6,0.1 -1.1,0.4 -1.5,0.8 -0.3,0.5 -0.5,1 -0.4,1.6 z" />
                </g>
                <path id={`${uid}tux-beak_base`} fill={`url(#${uid}tux-fill_beak_base)`} d="m 91.7,58.5 c 1.5,-1.7 2.6,-3.8 4,-5.6 0.7,-0.9 1.6,-1.7 2.6,-2.3 1,-0.6 2.1,-0.9 3.3,-0.8 1.3,0.1 2.5,0.7 3.4,1.6 0.9,0.9 1.6,2 2.2,3.1 0.5,1.1 0.9,2.3 1.7,3.2 0.8,1 1.9,1.7 3,2.5 0.5,0.4 1,0.8 1.4,1.3 0.4,0.5 0.7,1.1 0.9,1.7 0.1,0.7 0.1,1.3 -0.2,1.9 -0.2,0.6 -0.6,1.2 -1.1,1.6 -0.9,0.9 -2.2,1.4 -3.6,1.6 -2.6,0.4 -5.3,-0.4 -7.9,-0.3 -2.7,0.1 -5.3,1 -8,0.9 -1.3,-0.1 -2.7,-0.4 -3.8,-1.1 -0.6,-0.4 -1,-0.9 -1.4,-1.4 -0.3,-0.6 -0.6,-1.2 -0.6,-1.9 0,-0.6 0.1,-1.3 0.4,-1.9 0.2,-0.6 0.6,-1.1 1,-1.6 0.8,-1 1.8,-1.8 2.6,-2.8 z" />
                <g id={`${uid}tux-mandible_lower`}>
                  <path id={`${uid}tux-mandible_lower_base`} fill={`url(#${uid}tux-fill_mandible_lower_base)`} d="m 77.1,75 c 0.1,0.3 0.1,0.5 0.3,0.7 0.2,0.4 0.6,0.7 0.9,0.9 0.4,0.3 0.8,0.5 1.1,0.7 2,1.3 3.6,3 5.1,4.9 1.9,2.5 3.7,5.3 6.3,7.1 1.8,1.3 4,2 6.3,2.3 2.6,0.3 5.3,-0.2 7.8,-0.9 2.4,-0.7 4.6,-1.7 6.7,-3 4,-2.4 7.3,-6 11.7,-7.6 0.9,-0.3 1.9,-0.6 2.9,-1 0.9,-0.4 1.8,-1.1 2.2,-2 0.4,-0.9 0.4,-1.9 0.5,-2.9 0.1,-1 0.5,-2 0.7,-3.1 0.2,-1 0.2,-2.1 -0.2,-3.1 -0.4,-0.8 -1,-1.4 -1.8,-1.8 -0.8,-0.4 -1.6,-0.6 -2.5,-0.6 -1.7,-0.1 -3.4,0.4 -5.2,0.5 -2.3,0.2 -4.6,-0.1 -6.9,0 -2.9,0.1 -5.7,0.7 -8.5,0.9 -3.2,0.1 -6.5,-0.3 -9.7,-0.5 -1.4,-0.1 -2.8,0 -4.2,0.2 -1.4,0.2 -2.8,0.7 -3.9,1.5 -1.1,0.8 -2,1.8 -3.1,2.7 -0.5,0.5 -1.1,0.9 -1.7,1.2 -0.6,0.3 -1.3,0.5 -2,0.5 -0.3,0 -0.7,-0.1 -1.1,0 -0.2,0.1 -0.4,0.2 -0.6,0.3 -0.2,0.2 -0.3,0.3 -0.4,0.5 -0.3,0.4 -0.5,0.9 -0.8,1.3 z" />
                  <path id={`${uid}tux-mandible_lower_glare`} fill="#d9b30d" clipPath={`url(#${uid}tux-clip_mandible_lower)`} d="m 89.9,78.6 c -0.3,1.4 -0.1,2.9 0.6,4.1 0.7,1.2 1.8,2.2 3.2,2.6 1.7,0.6 3.6,0.3 5.2,-0.5 0.9,-0.5 1.8,-1.2 2.3,-2.1 0.3,-0.4 0.5,-0.9 0.5,-1.5 0.1,-0.5 0,-1.1 -0.2,-1.5 -0.2,-0.5 -0.6,-1 -1,-1.4 -0.4,-0.4 -0.9,-0.6 -1.5,-0.8 -1.1,-0.4 -2.8,-0.7 -3.9,-0.7 -2,0 -4.9,0.3 -5.2,1.7 z" />
                </g>
                <g id={`${uid}tux-mandible_upper`}>
                  <path id={`${uid}tux-mandible_upper_shadow`} fill="#604405" clipPath={`url(#${uid}tux-clip_mandible_lower)`} d="m 84.3,67.9 c -1.2,0.7 -2.3,1.4 -3.4,2.2 -0.6,0.4 -1.1,0.8 -1.4,1.5 -0.2,0.4 -0.3,0.9 -0.3,1.4 0,0.5 0,1 0,1.5 0,0.3 -0.1,0.7 -0.1,1 0,0.2 0,0.3 0,0.5 0.1,0.2 0.1,0.3 0.2,0.4 0.1,0.2 0.3,0.3 0.6,0.3 0.2,0.1 0.4,0.1 0.6,0.1 1,0.2 1.9,0.9 2.7,1.6 0.8,0.7 1.5,1.5 2.3,2.1 2.2,1.8 5.1,2.5 7.9,2.6 2.8,0.1 5.6,-0.5 8.4,-1.2 2.2,-0.5 4.3,-1 6.4,-1.8 3.2,-1.2 6.2,-3 8.7,-5.2 1.2,-1 2.3,-2.1 3.6,-2.9 1.1,-0.7 2.4,-1.2 3.6,-1.9 0.1,-0.1 0.2,-0.1 0.3,-0.2 0.1,-0.1 0.2,-0.2 0.2,-0.3 0.1,-0.2 0.1,-0.4 0,-0.6 -0.1,-0.2 -0.2,-0.4 -0.3,-0.6 -0.1,-0.2 -0.3,-0.3 -0.5,-0.5 -1.2,-1.1 -2.9,-1.4 -4.5,-1.6 -1.6,-0.1 -3.2,0 -4.8,-0.3 -1.5,-0.2 -2.9,-0.8 -4.4,-1.3 -1.5,-0.5 -3.1,-0.9 -4.6,-1.1 -3.7,-0.6 -7.5,-0.5 -11.2,0.3 -3.5,0.7 -6.9,2 -9.9,3.9 z" />
                  <path id={`${uid}tux-mandible_upper_base`} fill={`url(#${uid}tux-fill_mandible_upper_base)`} d="m 83.9,64 c -1.7,1.1 -3.2,2.5 -4.4,4 -0.7,0.9 -1.4,1.9 -1.7,2.9 -0.3,0.9 -0.4,1.8 -0.6,2.6 -0.1,0.3 -0.1,0.7 -0.1,1 0,0.2 0,0.3 0.1,0.5 0.1,0.2 0.1,0.3 0.2,0.4 0.2,0.2 0.5,0.3 0.7,0.4 0.3,0.1 0.6,0.1 0.8,0.1 1.2,0.1 2.4,0.8 3.4,1.5 1,0.7 2,1.6 3.1,2.2 2.4,1.5 5.2,2.1 8,2.3 2.8,0.1 5.6,-0.3 8.3,-0.9 2.2,-0.4 4.3,-0.9 6.4,-1.8 3.2,-1.2 6,-3.2 8.7,-5.2 1.2,-0.9 2.5,-1.9 3.6,-2.9 0.4,-0.4 0.7,-0.7 1.1,-1.1 0.4,-0.3 0.8,-0.6 1.4,-0.8 0.8,-0.2 1.6,-0.2 2.4,0 0.6,0.1 1.2,0.2 1.8,0.2 0.3,0 0.6,-0.1 0.9,-0.2 0.3,-0.1 0.5,-0.3 0.7,-0.5 0.2,-0.3 0.4,-0.7 0.4,-1.1 0,-0.4 -0.1,-0.8 -0.3,-1.1 -0.4,-0.7 -1.1,-1.1 -1.8,-1.5 -1,-0.4 -2.1,-0.7 -3.1,-0.9 -3.2,-0.8 -6.4,-1.9 -9.3,-3.4 -1.5,-0.7 -2.9,-1.5 -4.4,-2.3 -1.5,-0.8 -3,-1.6 -4.6,-2.1 -3.6,-1.1 -7.6,-0.7 -11.2,0.5 -4,1.4 -7.6,3.8 -10.3,7 z" />
                  <path id={`${uid}tux-mandible_upper_glare`} fill="#f6da4a" clipPath={`url(#${uid}tux-clip_mandible_upper)`} d="m 109.5,64.8 c -0.2,-0.2 -0.5,-0.4 -0.8,-0.5 -0.3,-0.1 -0.6,-0.1 -0.9,0 -0.6,0.1 -1.2,0.4 -1.7,0.8 -1.5,1 -2.8,2.3 -3.9,3.6 -1.5,1.8 -2.7,3.9 -3.2,6.3 -0.1,0.4 -0.1,0.8 -0.1,1.2 0.1,0.4 0.2,0.8 0.5,1.1 0.2,0.2 0.6,0.4 0.9,0.5 0.3,0.1 0.7,0 1,-0.1 0.7,-0.2 1.2,-0.7 1.7,-1.1 2.9,-2.6 5.5,-5.7 6.5,-9.4 0.1,-0.4 0.2,-0.8 0.2,-1.1 0,-0.4 -0.1,-0.8 -0.3,-1.1 z" />
                  <path id={`${uid}tux-naris_left`} opacity="0.8" fill={`url(#${uid}tux-fill_naris_left)`} d="m 92.7,59.1 c -0.8,-0.2 -2,1.1 -1.6,1.8 0.1,0.2 0.5,0.4 0.7,0.3 0.3,-0.2 0.6,-0.9 1,-1.2 0.2,-0.2 0.2,-0.8 -0.1,-0.9 z" />
                  <path id={`${uid}tux-naris_right`} opacity="0.8" fill={`url(#${uid}tux-fill_naris_right)`} d="m 102.6,59.4 c 0.2,0.6 1.2,0.5 1.8,0.8 0.5,0.3 0.9,0.9 1.5,0.9 0.6,0 1.4,-0.2 1.5,-0.8 0.1,-0.7 -1,-1.2 -1.7,-1.5 -0.9,-0.3 -2,-0.5 -2.9,-0.1 -0.2,0.1 -0.4,0.4 -0.3,0.6 z" />
                </g>
                <path id={`${uid}tux-beak_corner`} fill={`url(#${uid}tux-fill_beak_corner)`} clipPath={`url(#${uid}tux-clip_beak)`} d="m 129.3,69.2 a 2.4,3.1 16.9 0 1 -2.8,3 2.4,3.1 16.9 0 1 -2.1,-3 2.4,3.1 16.9 0 1 2.8,-3 2.4,3.1 16.9 0 1 2.1,3 z" />
              </g>
            </g>
          </g>
        </g>
      </>
    ),
  },
  manual: {
    render: () => (
      <g fill="none" stroke="var(--text-muted)" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
        <path d="M14.7 6.3a4 4 0 0 0-5.4 5.2l-5.2 5.2a1.6 1.6 0 0 0 2.3 2.3l5.2-5.2a4 4 0 0 0 5.2-5.4l-2.4 2.4-2.1-.5-.5-2.1Z" />
      </g>
    ),
  },
  akev: {
    render: () => (
      <g transform="translate(-4.94 -4.94) scale(0.03309)">
        <path d="M512 280 748 744H656L512 460.9 368 744H276Z" fill="var(--text)" stroke="var(--text)" strokeWidth="24" strokeLinejoin="round" />
        <circle cx="512" cy="684" r="62" fill="var(--accent)" />
      </g>
    ),
  },
  wireguard: {
    // The WireGuard mark (public-domain trace of wireguard.com/img/wireguard.svg, cropped
    // to the mark, dragon backed with white). WireGuard is a trademark of Jason A. Donenfeld.
    render: () => (
      <>
        <g transform="translate(2 2) scale(0.251969)">
          <circle cx="39.69" cy="39.69" r="38.9" fill="#fff" />
          <g strokeWidth="0.27" transform="translate(158.78 -78.3)">
            <path d="m-79.47 116.82s1.84-38.51-40.49-38.51c-37.43 0-38.6 36.94-38.6 36.94s-5.51 42.43 39.47 42.43c43.13 0 39.63-40.86 39.63-40.86zm-52.33-13.46c7.94-4.86 18.09-1.89 21.89 5.42 0.72 1.38 0.81 3.52 0.36 4.97-1.58 5.02-5.3 7.83-10.4 9.02 1.51-1.29 2.7-2.75 3.08-4.77a6.99 6.99 0 0 0 -1.2-5.54 7.08 7.08 0 0 0 -8.15-2.48c-3.14 1.19-4.87 4.06-4.56 7.59 0.29 3.28 2.77 5.4 7.42 6.21-0.7 0.37-1.23 0.64-1.75 0.93a16.91 16.91 0 0 0 -5.44 4.73c-0.47 0.64-0.8 0.69-1.52 0.25-9.35-5.72-9.95-20.07 0.26-26.31zm-7 35.33c-1.5 0.38-2.96 0.95-4.49 1.45 0.75-5.07 6.68-9.73 11.7-9.2a12.93 12.93 0 0 0 -2.45 6.85c-1.67 0.31-3.24 0.51-4.76 0.9zm31.96-49.47c1.48 0.05 2.97 0.03 4.46 0.07a7.69 7.69 0 0 1 1.1 0.15 10.74 10.74 0 0 1 -1.12 1.44c-0.53 0.49-1.13 0.98-1.9 0.23-0.18-0.18-0.62-0.14-0.94-0.14-1.48-0.02-2.96-0.07-4.43-0.01a27.53 27.53 0 0 0 -3.82 0.39c-0.24 0.04-0.59 0.83-0.48 1.12 0.26 0.68 0.63 1.44 1.18 1.88 2.05 1.62 4.23 3.07 6.28 4.67 2 1.56 3.86 3.27 4.99 5.62 1.48 3.07 1.52 6.28 0.88 9.51-1.06 5.39-3.79 9.86-8.21 13.1-1.78 1.31-3.98 2.05-6.02 2.99-1.79 0.83-3.64 1.54-5.44 2.36-3.24 1.47-5.06 4.99-4.53 8.65 0.49 3.36 3.44 6.16 6.81 6.74 4.05 0.69 8.22-1.94 9.21-6.05 1.11-4.62-1.4-8.75-6.1-10-0.21-0.06-0.41-0.11-0.85-0.22 1.26-0.56 2.34-0.96 3.35-1.51q2.63-1.44 5.15-3.06c0.5-0.32 0.76-0.32 1.19 0.05 3.23 2.8 5.16 6.28 5.71 10.54 0.9 7.06-2.45 13.55-8.75 16.87-9.75 5.14-21.69-0.71-23.84-11.52-1.85-9.26 4.69-17.66 12.56-19.28 3.38-0.7 6.48-2.11 8.88-4.71 1.55-1.68 2.3-3.12 2.56-3.77a10.47 10.47 0 0 0 0.72-3.83 8.96 8.96 0 0 0 -0.78-3.28c-0.82-1.87-3.97-4.85-4.75-5.48l-7.41-5.8c-0.26-0.21-0.56-0.2-1.19-0.16-0.76 0.05-2.69 0.16-3.53-0.06 0.68-0.51 2.52-1.26 3.31-1.85-2.4-1.62-5.14-1.04-7.66-1.52 0.58-1.08 3.46-2.75 5.1-2.93a24.22 24.22 0 0 0 -0.45-2.72c-0.1-0.37-0.51-0.73-0.87-0.94-0.87-0.51-1.79-0.93-2.79-1.44a5.8 5.8 0 0 1 3-0.93 11.2 11.2 0 0 1 3 0.29c1.78 0.41 3.21 0.14 4.63-1.07-1.12-0.45-2.23-0.86-3.32-1.35a32.55 32.55 0 0 1 -3.12-1.63c2.81 0.39 5.53 1.44 8.4 1.06q0.04-0.2 0.07-0.39c-2.15-0.5-4.3-1-6.68-1.55 3.98-0.36 7.68-0.42 11.19 1.28 0.99 0.48 2.02 0.88 2.97 1.43 0.46 0.27 0.77 0.8 1.15 1.21 0.3 0.33 0.54 0.76 0.91 0.96 1.4 0.75 2.95 0.77 4.52 0.74 0.01-0.18 0.02-0.35 0.03-0.53 1.58 0.49 3.36 2.32 3.36 3.65-2.56 0-5.13-0.01-7.69 0.01-0.27 0-0.55 0.2-0.82 0.31 0.26 0.15 0.51 0.42 0.78 0.43z" fill="#88171a" />
            <path d="m-110.15 85.42a0.39 0.39 0 0 0 -0.05 0.63 0.59 0.59 0 0 0 0.81 0.22c0.25-0.12 0.49-0.26 0.79-0.41-0.24-0.21-0.43-0.37-0.63-0.54-0.35-0.29-0.64-0.11-0.92 0.11z" />
          </g>
        </g>
      </>
    ),
  },
  app: {
    render: () => (
      <g fill="none" stroke="var(--text-muted)" strokeWidth="1.6">
        <rect x="3" y="4.5" width="18" height="15" rx="3" />
        <path d="M3 8.8h18" />
        <circle cx="6" cy="6.7" r="0.6" fill="var(--text-muted)" />
        <circle cx="8.2" cy="6.7" r="0.6" fill="var(--text-muted)" />
      </g>
    ),
  },
};
