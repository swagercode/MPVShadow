local mp = require('mp')

local text = mp.get_property('sub-text') -- string or nil

local start_s = mp.get_property_number('sub-start') -- number or nil
local end_s = mp.get_property_number('sub-end') -- number or nil

mp.observe_property('sub-text', "string", function(name, value)
    --value is the current rendered subtitle line (or nil if none)
    local start_s = mp.get_property_number('sub-start') -- number or nil
    local end_s = mp.get_property_number('sub-end') -- number or nil

    -- guard: if no line on screen, bail out
    if not value or not start_s or not end_s then
        return
    end
end)

mp.add_key_binding('c', 'analyzer-launcher', function()
    mp.commandv('script-message', 'cut_current_sub')
    
end)